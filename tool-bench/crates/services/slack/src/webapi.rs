//! Slack Web API methods (`/api/<method>`) over the same workspace store.
//! Arguments come from the query string, a form body, or a JSON body; replies use Slack's
//! `{"ok": true, ...}` / `{"ok": false, "error": "..."}` envelope with HTTP 200.

use serde_json::{Map, Value, json};

use crate::{
    model::{CALLER, Ctx, kind, real_name, ts_key},
    tools,
    util::{b, cursor, parse_cursor, s},
};

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Merges query-string, form and JSON arguments into one object of strings/values.
pub fn params(query: Option<&str>, content_type: &str, body: &[u8]) -> Map<String, Value> {
    let mut out = Map::new();
    let mut add_form = |text: &str| {
        for pair in text.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            out.insert(percent_decode(k), Value::String(percent_decode(v)));
        }
    };
    if let Some(q) = query {
        add_form(q);
    }
    if content_type.contains("json") {
        if let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(body) {
            out.extend(m);
        }
    } else if let Ok(text) = std::str::from_utf8(body) {
        add_form(text);
    }
    out
}

fn ok(mut extra: Value) -> Value {
    let mut m = Map::new();
    m.insert("ok".into(), json!(true));
    if let Some(o) = extra.as_object_mut() {
        m.extend(std::mem::take(o));
    }
    Value::Object(m)
}

fn fail(code: &str) -> Value {
    json!({"ok": false, "error": code})
}

fn p<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
}

fn p_int(args: &Map<String, Value>, key: &str) -> Option<i64> {
    match args.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(x) => x.parse().ok(),
        _ => None,
    }
}

pub fn channel_object(ctx: &Ctx, c: &Value) -> Value {
    let members = c["members"].as_array().map_or(0, Vec::len);
    let k = kind(c);
    let mut o = json!({
        "id": s(c, "id"),
        "created": c.get("created").cloned().unwrap_or(json!(0)),
        "is_archived": b(c, "is_archived"),
        "is_im": k == "im",
        "is_mpim": k == "mpim",
        "is_private": k != "public_channel",
        "is_channel": k == "public_channel" || k == "private_channel",
        "is_group": false,
        "is_member": ctx.is_member(c, CALLER),
        "context_team_id": ctx.team_id(),
    });
    if k == "im" {
        o["user"] = json!(s(c, "user"));
    } else {
        o["name"] = json!(s(c, "name"));
        o["name_normalized"] = json!(s(c, "name"));
        o["creator"] = json!(s(c, "creator"));
        o["topic"] = json!({"value": s(c, "topic"), "creator": "", "last_set": 0});
        o["purpose"] = json!({"value": s(c, "purpose"), "creator": "", "last_set": 0});
        o["num_members"] = json!(members);
    }
    o
}

pub fn message_object(ctx: &Ctx, m: &Value) -> Value {
    let mut o =
        json!({"type": "message", "user": s(m, "user"), "text": s(m, "text"), "ts": s(m, "ts")});
    if !s(m, "subtype").is_empty() {
        o["subtype"] = json!(s(m, "subtype"));
    }
    if !s(m, "bot_id").is_empty() {
        o["bot_id"] = json!(s(m, "bot_id"));
    }
    let channel = s(m, "channel");
    let replies = ctx.replies(channel, s(m, "ts"));
    if !s(m, "thread_ts").is_empty() {
        o["thread_ts"] = json!(s(m, "thread_ts"));
        if s(m, "thread_ts") != s(m, "ts") {
            o["parent_user_id"] = json!(
                ctx.message(channel, s(m, "thread_ts"))
                    .map(|x| s(&x, "user").to_owned())
                    .unwrap_or_default()
            );
        }
    }
    if !replies.is_empty() {
        o["thread_ts"] = json!(s(m, "ts"));
        o["reply_count"] = json!(replies.len());
        o["latest_reply"] = json!(s(replies.last().expect("non-empty"), "ts"));
        let mut users: Vec<String> = Vec::new();
        for r in &replies {
            let u = s(r, "user").to_owned();
            if !users.contains(&u) {
                users.push(u);
            }
        }
        o["reply_users"] = json!(users);
    }
    if let Some(r) = m
        .get("reactions")
        .filter(|r| r.as_array().is_some_and(|a| !a.is_empty()))
    {
        let list: Vec<Value> = r
            .as_array()
            .expect("array")
            .iter()
            .map(|x| json!({"name": s(x, "name"), "users": x["users"], "count": crate::model::reaction_count(x)}))
            .collect();
        o["reactions"] = json!(list);
    }
    if let Some(files) = m["files"].as_array() {
        let objs: Vec<Value> = files
            .iter()
            .filter_map(|id| ctx.get("files", id.as_str().unwrap_or("")))
            .map(|f| file_object(ctx, &f))
            .collect();
        o["files"] = json!(objs);
    }
    if b(m, "pinned") {
        o["pinned_to"] = json!([channel]);
    }
    o
}

pub fn user_object(ctx: &Ctx, u: &Value) -> Value {
    json!({
        "id": s(u, "id"),
        "team_id": ctx.team_id(),
        "name": s(u, "name"),
        "deleted": b(u, "deleted"),
        "real_name": real_name(u),
        "tz": s(u, "tz"),
        "is_admin": b(u, "is_admin"),
        "is_owner": b(u, "is_owner"),
        "is_bot": b(u, "is_bot"),
        "is_restricted": b(u, "is_restricted"),
        "profile": {
            "title": s(u, "title"),
            "phone": s(u, "phone"),
            "real_name": real_name(u),
            "display_name": s(u, "display_name"),
            "email": s(u, "email"),
            "status_text": s(u, "status_text"),
            "status_emoji": s(u, "status_emoji"),
            "pronouns": s(u, "pronouns"),
        },
    })
}

fn file_object(ctx: &Ctx, f: &Value) -> Value {
    json!({
        "id": s(f, "id"),
        "created": f.get("created").cloned().unwrap_or(json!(0)),
        "name": s(f, "name"),
        "title": s(f, "title"),
        "mimetype": s(f, "mimetype"),
        "filetype": s(f, "filetype"),
        "user": s(f, "user"),
        "size": f.get("size").cloned().unwrap_or(json!(0)),
        "channels": f.get("channels").cloned().unwrap_or(json!([])),
        "permalink": format!("{}/files/{}/{}/{}", ctx.base_url(), s(f, "user"), s(f, "id"), s(f, "name")),
        "preview": tools::file_preview(f),
    })
}

fn page<T: Clone>(items: &[T], args: &Map<String, Value>, default: usize) -> (Vec<T>, String) {
    let limit = p_int(args, "limit").map_or(default, |l| l.clamp(1, 1000) as usize);
    let offset: usize = p(args, "cursor")
        .and_then(|c| parse_cursor(c, "offset"))
        .and_then(|o| o.parse().ok())
        .unwrap_or(0);
    let next = if items.len() > offset + limit {
        cursor("offset", offset + limit)
    } else {
        String::new()
    };
    (
        items.iter().skip(offset).take(limit).cloned().collect(),
        next,
    )
}

pub fn call(ctx: &mut Ctx, method: &str, args: &Map<String, Value>) -> Value {
    match method {
        "api.test" => ok(json!({"args": args})),
        "auth.test" => {
            let team = ctx.team();
            let user = ctx
                .user(CALLER)
                .map(|u| s(&u, "name").to_owned())
                .unwrap_or_default();
            ok(json!({
                "url": format!("{}/", ctx.base_url()),
                "team": s(&team, "name"),
                "user": user,
                "team_id": s(&team, "id"),
                "user_id": CALLER,
                "is_enterprise_install": false,
            }))
        }
        "team.info" => {
            let team = ctx.team();
            ok(
                json!({"team": {"id": s(&team, "id"), "name": s(&team, "name"), "domain": s(&team, "domain")}}),
            )
        }
        "conversations.list" | "users.conversations" => {
            let types: Vec<String> = p(args, "types")
                .unwrap_or("public_channel")
                .split(',')
                .map(|t| t.trim().to_owned())
                .collect();
            let exclude_archived = p(args, "exclude_archived") == Some("true");
            let only_member = method == "users.conversations";
            let list: Vec<Value> = ctx
                .values("channels")
                .into_iter()
                .filter(|c| types.iter().any(|t| t == kind(c)) && ctx.can_see(c))
                .filter(|c| !(exclude_archived && b(c, "is_archived")))
                .filter(|c| !only_member || ctx.is_member(c, CALLER))
                .map(|c| channel_object(ctx, &c))
                .collect();
            let (items, next) = page(&list, args, 100);
            ok(json!({"channels": items, "response_metadata": {"next_cursor": next}}))
        }
        "conversations.info" => match p(args, "channel").map(|c| ctx.resolve_conversation(c)) {
            Some(Ok(c)) => ok(json!({"channel": channel_object(ctx, &c)})),
            Some(Err(e)) => fail(e),
            None => fail("channel_not_found"),
        },
        "conversations.history" => {
            let c = match p(args, "channel").map(|c| ctx.resolve_conversation(c)) {
                Some(Ok(c)) => c,
                Some(Err(e)) => return fail(e),
                None => return fail("channel_not_found"),
            };
            let oldest = p(args, "oldest").map(ts_key);
            let latest = p(args, "latest").map(ts_key);
            let mut msgs: Vec<Value> = ctx
                .messages_in(s(&c, "id"))
                .into_iter()
                .filter(|m| {
                    let t = s(m, "thread_ts");
                    t.is_empty() || t == s(m, "ts") || s(m, "subtype") == "thread_broadcast"
                })
                .filter(|m| {
                    let k = ts_key(s(m, "ts"));
                    oldest.is_none_or(|o| k > o) && latest.is_none_or(|l| k < l)
                })
                .collect();
            msgs.reverse();
            let objs: Vec<Value> = msgs.iter().map(|m| message_object(ctx, m)).collect();
            let (items, next) = page(&objs, args, 100);
            ok(json!({
                "messages": items,
                "has_more": !next.is_empty(),
                "pin_count": 0,
                "response_metadata": {"next_cursor": next},
            }))
        }
        "conversations.replies" => {
            let c = match p(args, "channel").map(|c| ctx.resolve_conversation(c)) {
                Some(Ok(c)) => c,
                Some(Err(e)) => return fail(e),
                None => return fail("channel_not_found"),
            };
            let Some(ts) = p(args, "ts") else {
                return fail("invalid_arguments");
            };
            let Some(parent) = ctx.message(s(&c, "id"), ts) else {
                return fail("thread_not_found");
            };
            let mut all = vec![message_object(ctx, &parent)];
            all.extend(
                ctx.replies(s(&c, "id"), ts)
                    .iter()
                    .map(|m| message_object(ctx, m)),
            );
            let (items, next) = page(&all, args, 1000);
            ok(
                json!({"messages": items, "has_more": !next.is_empty(), "response_metadata": {"next_cursor": next}}),
            )
        }
        "conversations.members" => match p(args, "channel").map(|c| ctx.resolve_conversation(c)) {
            Some(Ok(c)) => {
                let ids: Vec<Value> = c["members"].as_array().cloned().unwrap_or_default();
                let (items, next) = page(&ids, args, 100);
                ok(json!({"members": items, "response_metadata": {"next_cursor": next}}))
            }
            Some(Err(e)) => fail(e),
            None => fail("channel_not_found"),
        },
        "conversations.create" => {
            let mut a = json!({"channel_name": p(args, "name").unwrap_or("")});
            if p(args, "is_private") == Some("true") || args.get("is_private") == Some(&json!(true))
            {
                a["is_private"] = json!(true);
            }
            if p(args, "name").is_none() {
                return fail("invalid_name_required");
            }
            match tools::dispatch(ctx, "slack_create_conversation", &a) {
                Some(Ok(r)) => {
                    let c = ctx.channel(s(&r, "channel_id")).unwrap_or(Value::Null);
                    ok(json!({"channel": channel_object(ctx, &c)}))
                }
                Some(Err(e)) => fail(e.trim_start_matches("Error: ")),
                None => fail("unknown_method"),
            }
        }
        "chat.postMessage" => {
            let channel = p(args, "channel").unwrap_or("").to_owned();
            let text = p(args, "text").unwrap_or("").to_owned();
            let thread = p(args, "thread_ts").map(str::to_owned);
            let broadcast = p(args, "reply_broadcast") == Some("true")
                || args.get("reply_broadcast") == Some(&json!(true));
            match ctx.post_message(&channel, &text, thread.as_deref(), broadcast, Vec::new()) {
                Ok(m) => ok(
                    json!({"channel": s(&m, "channel"), "ts": s(&m, "ts"), "message": message_object(ctx, &m)}),
                ),
                Err(e) => fail(e),
            }
        }
        "chat.scheduleMessage" => {
            let a = json!({
                "channel_id": p(args, "channel").unwrap_or(""),
                "message": p(args, "text").unwrap_or(""),
                "post_at": p_int(args, "post_at").unwrap_or(0),
                "thread_ts": p(args, "thread_ts").unwrap_or(""),
            });
            match tools::dispatch(ctx, "slack_schedule_message", &a) {
                Some(Ok(_)) => {
                    let last = ctx
                        .values("scheduled_messages")
                        .pop()
                        .unwrap_or(Value::Null);
                    ok(
                        json!({"channel": s(&last, "channel"), "scheduled_message_id": s(&last, "id"), "post_at": last["post_at"]}),
                    )
                }
                Some(Err(e)) => fail(
                    e.trim_start_matches("Error: ")
                        .split(':')
                        .next()
                        .unwrap_or(""),
                ),
                None => fail("unknown_method"),
            }
        }
        "chat.scheduledMessages.list" => {
            let list: Vec<Value> = ctx
                .values("scheduled_messages")
                .into_iter()
                .map(|m| json!({"id": s(&m, "id"), "channel_id": s(&m, "channel"), "post_at": m["post_at"], "date_created": m["date_created"], "text": s(&m, "text")}))
                .collect();
            ok(json!({"scheduled_messages": list, "response_metadata": {"next_cursor": ""}}))
        }
        "chat.getPermalink" => {
            let (Some(channel), Some(ts)) = (p(args, "channel"), p(args, "message_ts")) else {
                return fail("invalid_arguments");
            };
            match ctx.message(channel, ts) {
                Some(m) => ok(json!({"channel": channel, "permalink": ctx.message_permalink(&m)})),
                None => fail("message_not_found"),
            }
        }
        "reactions.add" => {
            let channel = p(args, "channel").unwrap_or("").to_owned();
            let ts = p(args, "timestamp").unwrap_or("").to_owned();
            let name = p(args, "name").unwrap_or("").to_owned();
            match ctx.add_reaction(&channel, &ts, &name) {
                Ok(_) => ok(json!({})),
                Err(e) => fail(e),
            }
        }
        "reactions.get" => {
            let channel = p(args, "channel").unwrap_or("");
            let ts = p(args, "timestamp").unwrap_or("");
            match ctx.message(channel, ts) {
                Some(m) => ok(
                    json!({"type": "message", "channel": channel, "message": message_object(ctx, &m)}),
                ),
                None => fail("message_not_found"),
            }
        }
        "users.list" => {
            let list: Vec<Value> = ctx
                .values("users")
                .iter()
                .map(|u| user_object(ctx, u))
                .collect();
            let (items, next) = page(&list, args, 200);
            ok(
                json!({"members": items, "cache_ts": ctx.now, "response_metadata": {"next_cursor": next}}),
            )
        }
        "users.info" => match p(args, "user").and_then(|u| ctx.user(u)) {
            Some(u) => ok(json!({"user": user_object(ctx, &u)})),
            None => fail("user_not_found"),
        },
        "users.lookupByEmail" => {
            let email = p(args, "email").unwrap_or("").to_lowercase();
            match ctx
                .values("users")
                .into_iter()
                .find(|u| s(u, "email").to_lowercase() == email)
            {
                Some(u) => ok(json!({"user": user_object(ctx, &u)})),
                None => fail("users_not_found"),
            }
        }
        "users.profile.get" => {
            let id = p(args, "user").unwrap_or(CALLER);
            match ctx.user(id) {
                Some(u) => ok(json!({"profile": user_object(ctx, &u)["profile"]})),
                None => fail("user_not_found"),
            }
        }
        "emoji.list" => {
            let mut emoji = Map::new();
            for e in ctx.values("emojis") {
                let v = match s(&e, "alias_for") {
                    "" => s(&e, "url").to_owned(),
                    a => format!("alias:{a}"),
                };
                emoji.insert(s(&e, "name").to_owned(), json!(v));
            }
            ok(json!({"emoji": emoji, "cache_ts": "0"}))
        }
        "files.info" => match p(args, "file").and_then(|f| ctx.get("files", f)) {
            Some(f) => ok(json!({"file": file_object(ctx, &f)})),
            None => fail("file_not_found"),
        },
        "search.messages" => {
            let query = p(args, "query").unwrap_or("").to_owned();
            if query.trim().is_empty() {
                return fail("no_query");
            }
            let mut keywords = Vec::new();
            let mut filters = Vec::new();
            for tok in query.split_whitespace() {
                if tok.contains(':') && !tok.starts_with("http") {
                    filters.push(tok.to_owned());
                } else {
                    keywords.push(tok.to_owned());
                }
            }
            let a = json!({"keywords": keywords, "filters": filters.join(" "), "content_types": "messages", "limit": 20, "sort": if p(args, "sort") == Some("timestamp") { "timestamp" } else { "score" }});
            let docs = match crate::search::matching_messages(ctx, &a) {
                Ok(d) => d,
                Err(_) => return fail("invalid_query"),
            };
            let matches: Vec<Value> = docs
                .iter()
                .map(|m| {
                    let c = ctx.channel(s(m, "channel")).unwrap_or(Value::Null);
                    let mut o = message_object(ctx, m);
                    o["channel"] = json!({"id": s(&c, "id"), "name": s(&c, "name")});
                    o["permalink"] = json!(ctx.message_permalink(m));
                    o
                })
                .collect();
            let total = matches.len();
            ok(json!({
                "query": query,
                "messages": {"total": total, "matches": matches, "paging": {"count": 20, "total": total, "page": 1, "pages": total.div_ceil(20).max(1)}},
            }))
        }
        _ => fail("unknown_method"),
    }
}
