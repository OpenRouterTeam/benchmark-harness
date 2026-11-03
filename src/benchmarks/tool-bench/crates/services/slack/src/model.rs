//! The Slack workspace store and the operations shared by the MCP tools and the Web API.
//!
//! Collections (all optional in a dataset):
//! - `team`      one document keyed `team`: `{id, name, domain}`
//! - `users`     keyed by user ID
//! - `channels`  keyed by conversation ID (`C…`, `G…`, `D…`); `members` is an array of user IDs
//! - `messages`  keyed `<channel>/<ts>`
//! - `posted`    keyed `<channel>#<n>`: every message the caller posted through a tool, in order
//! - `files`, `canvases`, `lists`, `drafts`, `scheduled_messages`, `emojis`

use mock_core::world::ServiceState;
use serde_json::{Value, json};

use crate::{
    tz,
    util::{b, s, slack_id},
};

pub const CALLER: &str = "U0TOOLBNCH1";

pub struct Ctx<'a> {
    pub st: &'a mut ServiceState,
    /// Logical unix time of this request.
    pub now: i64,
}

impl Ctx<'_> {
    pub fn all(&self, coll: &str) -> Vec<(String, Value)> {
        self.st
            .store
            .collection(coll)
            .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn values(&self, coll: &str) -> Vec<Value> {
        self.st
            .store
            .collection(coll)
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get(&self, coll: &str, key: &str) -> Option<Value> {
        self.st.store.collection(coll)?.get(key).cloned()
    }

    pub fn put(&mut self, coll: &str, key: &str, doc: Value) {
        self.st
            .store
            .collection_mut(coll)
            .insert(key.to_owned(), doc);
    }

    pub fn remove(&mut self, coll: &str, key: &str) -> Option<Value> {
        self.st.store.collection_mut(coll).remove(key)
    }

    pub fn team(&self) -> Value {
        self.get("team", "team").unwrap_or_else(
            || json!({"id": "T0TOOLBNCH", "name": "Octo Corp", "domain": "octo-corp"}),
        )
    }

    pub fn team_id(&self) -> String {
        s(&self.team(), "id").to_owned()
    }

    pub fn base_url(&self) -> String {
        let team = self.team();
        let domain = match s(&team, "domain") {
            "" => "octo-corp",
            d => d,
        };
        format!("https://{domain}.slack.com")
    }

    pub fn user(&self, id: &str) -> Option<Value> {
        self.get("users", id)
    }

    pub fn caller_tz(&self) -> String {
        self.user(CALLER)
            .map(|u| s(&u, "tz").to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "America/Chicago".to_owned())
    }

    pub fn time(&self, ts: &str) -> String {
        tz::format_local(&self.caller_tz(), ts_secs(ts))
    }

    pub fn channel(&self, id: &str) -> Option<Value> {
        self.get("channels", id)
    }

    pub fn is_member(&self, channel: &Value, user: &str) -> bool {
        channel
            .get("members")
            .and_then(Value::as_array)
            .is_some_and(|m| m.iter().any(|x| x == user))
    }

    /// The caller can see this conversation (public, or a member).
    pub fn can_see(&self, channel: &Value) -> bool {
        kind(channel) == "public_channel" || self.is_member(channel, CALLER)
    }

    /// Resolves a channel ID, or a user ID to the caller's DM with that user.
    pub fn resolve_conversation(&self, id: &str) -> Result<Value, &'static str> {
        let id = id.trim().trim_start_matches('#');
        if id.starts_with('U') || id.starts_with('W') {
            if self.user(id).is_none() {
                return Err("user_not_found");
            }
            return self
                .values("channels")
                .into_iter()
                .find(|c| b(c, "is_im") && s(c, "user") == id)
                .ok_or("channel_not_found");
        }
        let channel = self
            .channel(id)
            .or_else(|| {
                self.values("channels")
                    .into_iter()
                    .find(|c| !b(c, "is_im") && s(c, "name").eq_ignore_ascii_case(id))
            })
            .ok_or("channel_not_found")?;
        if !self.can_see(&channel) {
            return Err("channel_not_found");
        }
        Ok(channel)
    }

    pub fn messages_in(&self, channel: &str) -> Vec<Value> {
        let mut out: Vec<Value> = self
            .values("messages")
            .into_iter()
            .filter(|m| s(m, "channel") == channel)
            .collect();
        out.sort_by_key(|a| ts_key(s(a, "ts")));
        out
    }

    pub fn message(&self, channel: &str, ts: &str) -> Option<Value> {
        self.get("messages", &format!("{channel}/{ts}"))
    }

    pub fn replies(&self, channel: &str, parent_ts: &str) -> Vec<Value> {
        self.messages_in(channel)
            .into_iter()
            .filter(|m| s(m, "thread_ts") == parent_ts && s(m, "ts") != parent_ts)
            .collect()
    }

    /// A new message timestamp at the current logical second.
    pub fn new_ts(&mut self) -> String {
        let n = self.st.ids.next_int("message_ts", 1);
        let micros = (n * 7919 + 104_729) % 1_000_000;
        format!("{}.{micros:06}", self.now)
    }

    pub fn new_id(&mut self, kind: &str, prefix: &str) -> String {
        slack_id(self.st, kind, prefix, 9)
    }

    pub fn permalink(&self, channel: &str, ts: &str, thread_ts: Option<&str>) -> String {
        let p = ts.replace('.', "");
        match thread_ts {
            Some(t) if !t.is_empty() => format!(
                "{}/archives/{channel}/p{p}?thread_ts={t}&cid={channel}",
                self.base_url()
            ),
            _ => format!("{}/archives/{channel}/p{p}", self.base_url()),
        }
    }

    /// Permalink for a message document (threads get the `thread_ts` form).
    pub fn message_permalink(&self, m: &Value) -> String {
        let channel = s(m, "channel");
        let ts = s(m, "ts");
        let thread = match s(m, "thread_ts") {
            "" if self.replies(channel, ts).is_empty() => None,
            "" => Some(ts),
            t => Some(t),
        };
        self.permalink(channel, ts, thread)
    }

    pub fn channel_label(&self, channel: &Value) -> String {
        if b(channel, "is_im") {
            let other = self.user(s(channel, "user"));
            let name = other.as_ref().map(display_name).unwrap_or_default();
            format!("DM with {name}")
        } else {
            format!("#{}", s(channel, "name"))
        }
    }

    /// Posts a message as the caller. Returns the stored message.
    pub fn post_message(
        &mut self,
        channel_id: &str,
        text: &str,
        thread_ts: Option<&str>,
        broadcast: bool,
        files: Vec<String>,
    ) -> Result<Value, &'static str> {
        let channel = match self.resolve_conversation(channel_id) {
            Err("channel_not_found") if channel_id.starts_with('U') => {
                // Posting to a user ID opens the DM, as chat.postMessage does.
                let user = self.user(channel_id).ok_or("user_not_found")?;
                if b(&user, "is_bot") {
                    return Err("cannot_dm_bot");
                }
                let id = self.new_id("im", "D0");
                let doc = json!({"id": id, "is_im": true, "user": channel_id, "created": self.now, "members": [CALLER, channel_id]});
                self.put("channels", &id, doc.clone());
                doc
            }
            other => other?,
        };
        let channel_id = s(&channel, "id").to_owned();
        if b(&channel, "is_archived") {
            return Err("is_archived");
        }
        if !self.is_member(&channel, CALLER) {
            return Err("not_in_channel");
        }
        if text.trim().is_empty() && files.is_empty() {
            return Err("no_text");
        }
        let thread_ts = thread_ts.filter(|t| !t.is_empty());
        if let Some(t) = thread_ts {
            let parent = self.message(&channel_id, t).ok_or("thread_not_found")?;
            // Replies to a reply attach to the thread parent, as in Slack.
            if !s(&parent, "thread_ts").is_empty() && s(&parent, "thread_ts") != t {
                return self.post_message(
                    &channel_id,
                    text,
                    Some(s(&parent, "thread_ts")),
                    broadcast,
                    files,
                );
            }
        }
        let ts = self.new_ts();
        let mut msg = json!({
            "channel": channel_id,
            "ts": ts,
            "user": CALLER,
            "text": text,
        });
        if let Some(t) = thread_ts {
            msg["thread_ts"] = json!(t);
            if broadcast {
                msg["subtype"] = json!("thread_broadcast");
            }
        }
        if !files.is_empty() {
            msg["files"] = json!(files);
        }
        self.put("messages", &format!("{channel_id}/{ts}"), msg.clone());
        let n = self
            .all("posted")
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{channel_id}#")))
            .count()
            + 1;
        self.put("posted", &format!("{channel_id}#{n}"), msg.clone());
        Ok(msg)
    }

    /// Adds the caller's reaction. Duplicates succeed silently.
    pub fn add_reaction(
        &mut self,
        channel_id: &str,
        ts: &str,
        emoji: &str,
    ) -> Result<Value, &'static str> {
        let channel = self.resolve_conversation(channel_id)?;
        let channel_id = s(&channel, "id").to_owned();
        let key = format!("{channel_id}/{ts}");
        let mut msg = self.get("messages", &key).ok_or("message_not_found")?;
        let name = emoji.trim().trim_matches(':').to_owned();
        if name.is_empty() {
            return Err("invalid_name");
        }
        if !is_known_emoji(self, &name) {
            return Err("invalid_name");
        }
        let reactions = msg
            .as_object_mut()
            .expect("message object")
            .entry("reactions")
            .or_insert_with(|| json!([]));
        let list = reactions.as_array_mut().expect("reactions array");
        if let Some(r) = list.iter_mut().find(|r| s(r, "name") == name) {
            let already = r["users"]
                .as_array()
                .is_some_and(|u| u.iter().any(|x| x == CALLER));
            if !already {
                let before = reaction_count(r);
                let obj = r.as_object_mut().expect("reaction object");
                obj.entry("users")
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .expect("users array")
                    .push(json!(CALLER));
                obj.insert("count".into(), json!(before + 1));
            }
        } else {
            list.push(json!({"name": name, "users": [CALLER], "count": 1}));
        }
        self.put("messages", &key, msg.clone());
        Ok(msg)
    }
}

pub fn ts_secs(ts: &str) -> i64 {
    ts.split('.')
        .next()
        .and_then(|x| x.parse().ok())
        .unwrap_or(0)
}

/// Sortable key for a Slack ts.
pub fn ts_key(ts: &str) -> (i64, i64) {
    let mut parts = ts.split('.');
    let a = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let b = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (a, b)
}

/// `public_channel`, `private_channel`, `im`, or `mpim`.
pub fn kind(channel: &Value) -> &'static str {
    if b(channel, "is_im") {
        "im"
    } else if b(channel, "is_mpim") {
        "mpim"
    } else if b(channel, "is_private") {
        "private_channel"
    } else {
        "public_channel"
    }
}

pub fn display_name(user: &Value) -> String {
    match s(user, "display_name") {
        "" => s(user, "real_name").to_owned(),
        d => d.to_owned(),
    }
}

pub fn real_name(user: &Value) -> String {
    match s(user, "real_name") {
        "" => s(user, "name").to_owned(),
        r => r.to_owned(),
    }
}

pub fn reaction_count(r: &Value) -> u64 {
    r.get("count")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| r["users"].as_array().map_or(0, |u| u.len() as u64))
}

/// Standard emoji names the mock recognises, plus the workspace's custom ones.
const STANDARD_EMOJI: &[&str] = &[
    "+1",
    "-1",
    "thumbsup",
    "thumbsdown",
    "white_check_mark",
    "heavy_check_mark",
    "x",
    "eyes",
    "tada",
    "rocket",
    "fire",
    "heart",
    "pray",
    "raised_hands",
    "clap",
    "100",
    "joy",
    "smile",
    "slightly_smiling_face",
    "thinking_face",
    "warning",
    "rotating_light",
    "ticket",
    "memo",
    "calendar",
    "point_up",
    "wave",
    "ok_hand",
    "muscle",
    "bug",
    "hourglass",
    "lock",
    "star",
    "sparkles",
    "zap",
    "ship",
    "bell",
    "question",
    "exclamation",
    "red_circle",
    "large_green_circle",
    "large_yellow_circle",
    "white_circle",
    "speech_balloon",
    "pushpin",
    "link",
    "inbox_tray",
    "outbox_tray",
    "chart_with_upwards_trend",
    "money_with_wings",
    "partying_face",
    "sob",
    "sweat_smile",
    "see_no_evil",
    "facepalm",
    "shrug",
    "coffee",
    "pizza",
    "beers",
    "trophy",
    "medal",
    "construction",
    "no_entry",
    "stop_sign",
    "sos",
];

pub fn is_known_emoji(ctx: &Ctx, name: &str) -> bool {
    let base = name.split("::").next().unwrap_or(name);
    STANDARD_EMOJI.contains(&base) || ctx.get("emojis", base).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_ordering() {
        assert!(ts_key("1793034643.000010") > ts_key("1793034643.000002"));
        assert_eq!(ts_secs("1793034643.464039"), 1_793_034_643);
    }
}
