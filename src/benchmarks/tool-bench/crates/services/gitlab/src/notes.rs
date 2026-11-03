//! Notes, discussions and award emoji on issues and merge requests, plus the events and
//! system-note helpers other modules use.
//!
//! Collections: `notes` (id-keyed, `noteable` = issue/MR key), `award_emoji` (id-keyed,
//! `awardable` = issue/MR key or `note:<id>`), `events` (id-keyed).

use axum::{Router, http::StatusCode, response::Response, routing::get as rget};
use mock_core::{
    store::Store,
    world::{SharedWorld, WorldState},
};
use serde_json::{Value, json};

use crate::common::*;

pub fn routes() -> Router<SharedWorld> {
    let mut router = Router::new();
    for kind in ["issues", "merge_requests"] {
        let b = format!("/projects/{{id}}/{kind}/{{iid}}");
        router = router
            .route(
                &format!("{b}/notes"),
                rget(wrap(list_notes)).post(wrap(create_note)),
            )
            .route(
                &format!("{b}/notes/{{note_id}}"),
                rget(wrap(get_note))
                    .put(wrap(update_note))
                    .delete(wrap(delete_note)),
            )
            .route(
                &format!("{b}/discussions"),
                rget(wrap(list_discussions)).post(wrap(create_discussion)),
            )
            .route(
                &format!("{b}/discussions/{{discussion_id}}"),
                rget(wrap(get_discussion)).put(wrap(resolve_discussion)),
            )
            .route(
                &format!("{b}/discussions/{{discussion_id}}/notes"),
                axum::routing::post(wrap(reply_to_discussion)),
            )
            .route(
                &format!("{b}/discussions/{{discussion_id}}/notes/{{note_id}}"),
                rget(wrap(get_note))
                    .put(wrap(update_discussion_note))
                    .delete(wrap(delete_note)),
            )
            .route(
                &format!("{b}/award_emoji"),
                rget(wrap(list_awards)).post(wrap(create_award)),
            )
            .route(
                &format!("{b}/award_emoji/{{award_id}}"),
                rget(wrap(get_award)).delete(wrap(delete_award)),
            )
            .route(
                &format!("{b}/notes/{{note_id}}/award_emoji"),
                rget(wrap(list_awards)).post(wrap(create_award)),
            )
            .route(
                &format!("{b}/notes/{{note_id}}/award_emoji/{{award_id}}"),
                rget(wrap(get_award)).delete(wrap(delete_award)),
            )
            .route(
                &format!("{b}/discussions/{{discussion_id}}/notes/{{note_id}}/award_emoji"),
                rget(wrap(list_awards)).post(wrap(create_award)),
            )
            .route(
                &format!(
                    "{b}/discussions/{{discussion_id}}/notes/{{note_id}}/award_emoji/{{award_id}}"
                ),
                rget(wrap(get_award)).delete(wrap(delete_award)),
            );
    }
    router
}

// ---------------------------------------------------------------------------
// Helpers used by other modules

/// Appends an event to the `events` collection.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_event(
    st: &mut Store,
    project: &str,
    author: &str,
    action: &str,
    target_type: &str,
    target_id: u64,
    target_iid: u64,
    title: &str,
    now: &str,
) {
    let id = next_id(st, "events", 1);
    st.collection_mut("events").insert(
        id.to_string(),
        json!({
            "id": id,
            "project": project,
            "author": author,
            "action_name": action,
            "target_type": target_type,
            "target_id": target_id,
            "target_iid": target_iid,
            "target_title": title,
            "created_at": now,
        }),
    );
}

/// Adds a system note (`"changed milestone to ..."`) to an issue or MR.
pub(crate) fn add_system_note(st: &mut Store, noteable: &str, author: &str, body: &str, now: &str) {
    let id = next_id(st, "notes", 1);
    st.collection_mut("notes").insert(
        id.to_string(),
        json!({
            "id": id,
            "noteable": noteable,
            "body": body,
            "author": author,
            "created_at": now,
            "updated_at": now,
            "system": true,
            "internal": false,
            "discussion_id": fake_sha(&format!("discussion:{id}")),
            "type": null,
            "resolvable": false,
            "resolved": false,
        }),
    );
}

fn noteable_doc<'a>(st: &'a Store, key: &str) -> Option<&'a Value> {
    if key.contains('!') {
        get(st, "merge_requests", key)
    } else {
        get(st, "issues", key)
    }
}

/// Full note JSON.
pub(crate) fn render_note(st: &Store, base: &str, n: &Value) -> Value {
    let noteable = str_of(n, "noteable");
    let is_mr = noteable.contains('!');
    let project = project_of(noteable);
    let target = noteable_doc(st, noteable);
    let resolvable = n
        .get("resolvable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let resolved = resolvable && n.get("resolved").and_then(Value::as_bool).unwrap_or(false);
    let mut out = json!({
        "id": n["id"],
        "type": n.get("type").cloned().unwrap_or(Value::Null),
        "body": str_of(n, "body"),
        "attachment": null,
        "author": user_basic(st, base, str_of(n, "author")),
        "created_at": n["created_at"],
        "updated_at": n.get("updated_at").unwrap_or(&n["created_at"]),
        "system": n.get("system").cloned().unwrap_or(json!(false)),
        "noteable_id": target.and_then(|t| as_u64(&t["id"])).unwrap_or(0),
        "noteable_type": if is_mr { "MergeRequest" } else { "Issue" },
        "project_id": project_id(st, project),
        "commit_id": null,
        "resolvable": resolvable,
        "confidential": n.get("internal").cloned().unwrap_or(json!(false)),
        "internal": n.get("internal").cloned().unwrap_or(json!(false)),
        "imported": false,
        "imported_from": "none",
        "noteable_iid": iid_of(noteable),
        "commands_changes": {},
    });
    if resolvable {
        out["resolved"] = json!(resolved);
        out["resolved_by"] = if resolved {
            user_opt(st, base, &n["resolved_by"])
        } else {
            Value::Null
        };
        out["resolved_at"] = if resolved {
            n.get("resolved_at").cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        };
    }
    if let Some(p) = n.get("position").filter(|p| !p.is_null()) {
        out["position"] = p.clone();
    }
    out
}

/// Notes of an issue/MR in insertion (chronological) order.
fn notes_of(st: &Store, noteable: &str) -> Vec<Value> {
    all(st, "notes")
        .into_iter()
        .filter(|(_, n)| str_of(n, "noteable") == noteable)
        .map(|(_, n)| n.clone())
        .collect()
}

/// Discussions (grouped by `discussion_id`, ordered by first note).
pub(crate) fn discussions_of(st: &Store, base: &str, noteable: &str) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<Value>> = Vec::new();
    for n in notes_of(st, noteable) {
        let did = n
            .get("discussion_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| fake_sha(&format!("discussion:{}", n["id"])));
        match order.iter().position(|d| *d == did) {
            Some(i) => groups[i].push(n),
            None => {
                order.push(did);
                groups.push(vec![n]);
            }
        }
    }
    order
        .into_iter()
        .zip(groups)
        .map(|(id, notes)| {
            let individual = notes.len() == 1 && notes[0].get("type").is_none_or(Value::is_null);
            json!({
                "id": id,
                "individual_note": individual,
                "notes": notes.iter().map(|n| render_note(st, base, n)).collect::<Vec<_>>(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Request plumbing

fn is_mr(r: &Req) -> bool {
    r.uri_path.contains("/merge_requests/")
}

/// The issue/MR key addressed by the request, or the 404.
fn require_noteable(st: &Store, r: &Req) -> Result<String, Response> {
    let project = require_project(st, r)?;
    let (sep, what) = if is_mr(r) {
        ('!', "Merge Request")
    } else {
        ('#', "Issue")
    };
    let key = format!("{project}{sep}{}", r.p("iid"));
    if noteable_doc(st, &key).is_some() {
        Ok(key)
    } else {
        Err(not_found(what))
    }
}

fn find_note(st: &Store, noteable: &str, id: &str) -> Option<Value> {
    get(st, "notes", id)
        .filter(|n| str_of(n, "noteable") == noteable)
        .cloned()
}

fn locked(st: &Store, noteable: &str) -> bool {
    noteable_doc(st, noteable)
        .and_then(|d| d.get("discussion_locked")?.as_bool())
        .unwrap_or(false)
}

fn insert_note(
    w: &mut WorldState,
    noteable: &str,
    body: &str,
    discussion: Option<(String, Option<String>)>,
    extra: Value,
    created_at: Option<String>,
) -> Value {
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "notes", 1);
    let author = current_username(st);
    let is_mr = noteable.contains('!');
    let (discussion_id, ty) = match discussion {
        Some((d, t)) => (d, t),
        None => (fake_sha(&format!("discussion:{id}")), None),
    };
    let resolvable = is_mr && ty.is_some();
    let mut doc = json!({
        "id": id,
        "noteable": noteable,
        "body": body,
        "author": author,
        "created_at": created_at.unwrap_or_else(|| now.clone()),
        "updated_at": now,
        "system": false,
        "internal": false,
        "discussion_id": discussion_id,
        "type": ty,
        "resolvable": resolvable,
        "resolved": false,
    });
    merge_into(&mut doc, extra);
    st.collection_mut("notes")
        .insert(id.to_string(), doc.clone());
    if let Some(d) = st
        .collection_mut(if is_mr { "merge_requests" } else { "issues" })
        .get_mut(noteable)
    {
        d["updated_at"] = json!(now);
    }
    let project = project_of(noteable).to_owned();
    let title = noteable_doc(st, noteable)
        .map(|d| str_of(d, "title").to_owned())
        .unwrap_or_default();
    record_event(
        st,
        &project,
        &author,
        "commented on",
        "Note",
        id,
        iid_of(noteable),
        &title,
        &now,
    );
    doc
}

// ---------------------------------------------------------------------------
// Notes

fn list_notes(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut notes = notes_of(st, &key);
    let field = if r.q("order_by") == Some("updated_at") {
        "updated_at"
    } else {
        "created_at"
    };
    notes.sort_by(|a, b| {
        let ka = (str_of(a, field).to_owned(), as_u64(&a["id"]));
        let kb = (str_of(b, field).to_owned(), as_u64(&b["id"]));
        if r.q("sort") == Some("asc") {
            ka.cmp(&kb)
        } else {
            kb.cmp(&ka)
        }
    });
    let items = notes
        .iter()
        .filter(|n| match r.q("activity_filter") {
            Some("only_comments") => !n["system"].as_bool().unwrap_or(false),
            Some("only_activity") => n["system"].as_bool().unwrap_or(false),
            _ => true,
        })
        .map(|n| render_note(st, &r.base, n))
        .collect();
    paged(r, items)
}

fn get_note(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    match find_note(st, &key, r.p("note_id")) {
        Some(n)
            if r.path
                .get("discussion_id")
                .is_none_or(|d| str_of(&n, "discussion_id") == d) =>
        {
            ok(render_note(st, &r.base, &n))
        }
        _ => not_found("Note"),
    }
}

fn create_note(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(body) = r.s("body").filter(|b| !b.trim().is_empty()) else {
        return missing("body");
    };
    if locked(st, &key) {
        return forbidden();
    }
    let internal = r
        .bool("internal")
        .or(r.bool("confidential"))
        .unwrap_or(false);
    let doc = insert_note(
        w,
        &key,
        &body,
        None,
        json!({"internal": internal}),
        r.s("created_at"),
    );
    created(render_note(store(w), &r.base, &doc))
}

fn update_note(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let id = r.p("note_id").to_owned();
    let Some(n) = find_note(st, &key, &id) else {
        return not_found("Note");
    };
    if n["system"].as_bool().unwrap_or(false) {
        return forbidden();
    }
    if !r.has("body") && !r.has("confidential") && !r.has("internal") {
        return missing("body");
    }
    let now = now(w);
    let st = svc_mut(w);
    let doc = st.collection_mut("notes").get_mut(&id).expect("note");
    if let Some(b) = r.s("body") {
        doc["body"] = json!(b);
    }
    doc["updated_at"] = json!(now);
    let doc = doc.clone();
    ok(render_note(st, &r.base, &doc))
}

fn update_discussion_note(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let id = r.p("note_id").to_owned();
    let Some(n) =
        find_note(st, &key, &id).filter(|n| str_of(n, "discussion_id") == r.p("discussion_id"))
    else {
        return not_found("Note");
    };
    let body = r.s("body");
    let resolved = r.bool("resolved");
    if body.is_none() && resolved.is_none() {
        return bad_request("body, resolved are missing, exactly one parameter must be provided");
    }
    if resolved.is_some() && !n["resolvable"].as_bool().unwrap_or(false) {
        return bad_request("Note is not resolvable");
    }
    let now = now(w);
    let st = svc_mut(w);
    let current = current_username(st);
    let doc = st.collection_mut("notes").get_mut(&id).expect("note");
    if let Some(b) = body {
        doc["body"] = json!(b);
        doc["updated_at"] = json!(now);
    }
    if let Some(res) = resolved {
        doc["resolved"] = json!(res);
        doc["resolved_by"] = if res { json!(current) } else { Value::Null };
        doc["resolved_at"] = if res { json!(now) } else { Value::Null };
    }
    let doc = doc.clone();
    ok(render_note(st, &r.base, &doc))
}

fn delete_note(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let id = r.p("note_id").to_owned();
    let Some(n) = find_note(st, &key, &id) else {
        return not_found("Note");
    };
    if let Some(d) = r.path.get("discussion_id")
        && str_of(&n, "discussion_id") != d
    {
        return not_found("Note");
    }
    if n["system"].as_bool().unwrap_or(false) {
        return forbidden();
    }
    remove_doc(svc_mut(w), "notes", &id);
    no_content()
}

// ---------------------------------------------------------------------------
// Discussions

fn list_discussions(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    paged(r, discussions_of(st, &r.base, &key))
}

fn find_discussion(st: &Store, base: &str, key: &str, id: &str) -> Option<Value> {
    discussions_of(st, base, key)
        .into_iter()
        .find(|d| d["id"] == id)
}

fn get_discussion(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    match find_discussion(st, &r.base, &key, r.p("discussion_id")) {
        Some(d) => ok(d),
        None => not_found("Discussion"),
    }
}

fn create_discussion(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(body) = r.s("body").filter(|b| !b.trim().is_empty()) else {
        return missing("body");
    };
    if locked(st, &key) {
        return forbidden();
    }
    let mut extra = json!({});
    let mut ty = "DiscussionNote";
    if let Some(pos) = r.body.get("position").filter(|p| p.is_object()) {
        if !is_mr(r) {
            return bad_request("position is not supported for issues");
        }
        let mut pos = pos.clone();
        if pos.get("position_type").is_none() {
            pos["position_type"] = json!("text");
        }
        let mr = get(st, "merge_requests", &key)
            .cloned()
            .unwrap_or(json!({}));
        let path = pos
            .get("new_path")
            .or(pos.get("old_path"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let changed = value_list(&json!(
            mr["changes"]
                .as_array()
                .map(|c| c
                    .iter()
                    .flat_map(|d| [d["new_path"].clone(), d["old_path"].clone()])
                    .collect::<Vec<_>>())
                .unwrap_or_default()
        ));
        if !path.is_empty() && !changed.contains(&path) {
            return json_status(
                StatusCode::BAD_REQUEST,
                json!({"message": "400 Bad request - Note {:line_code=>[\"can't be blank\", \"must be a valid line code\"]}"}),
            );
        }
        for f in ["base_sha", "start_sha", "head_sha"] {
            if pos.get(f).is_none() {
                pos[f] = json!(str_of(&mr, "sha"));
            }
        }
        extra["position"] = pos;
        ty = "DiffNote";
    }
    let did = fake_sha(&format!("thread:{key}:{}", next_id(store(w), "notes", 1)));
    insert_note(
        w,
        &key,
        &body,
        Some((did.clone(), Some(ty.into()))),
        extra,
        r.s("created_at"),
    );
    let st = store(w);
    match find_discussion(st, &r.base, &key, &did) {
        Some(d) => created(d),
        None => not_found("Discussion"),
    }
}

fn reply_to_discussion(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let did = r.p("discussion_id").to_owned();
    let Some(d) = find_discussion(st, &r.base, &key, &did) else {
        return not_found("Discussion");
    };
    let Some(body) = r
        .s("body")
        .or_else(|| r.s("note"))
        .filter(|b| !b.trim().is_empty())
    else {
        return missing("body");
    };
    if d["individual_note"].as_bool().unwrap_or(false) {
        // Replying to an individual note turns it into a thread.
        let first = d["notes"][0]["id"].to_string();
        if let Some(n) = svc_mut(w).collection_mut("notes").get_mut(&first) {
            n["type"] = json!("DiscussionNote");
            n["resolvable"] = json!(key.contains('!'));
        }
    }
    let ty = if d["notes"][0]["type"] == "DiffNote" {
        "DiffNote"
    } else {
        "DiscussionNote"
    };
    let doc = insert_note(
        w,
        &key,
        &body,
        Some((did, Some(ty.into()))),
        json!({}),
        r.s("created_at"),
    );
    created(render_note(store(w), &r.base, &doc))
}

fn resolve_discussion(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let key = match require_noteable(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    if !is_mr(r) {
        return route_not_found();
    }
    let did = r.p("discussion_id").to_owned();
    let Some(d) = find_discussion(st, &r.base, &key, &did) else {
        return not_found("Discussion");
    };
    let Some(resolved) = r.bool("resolved") else {
        return missing("resolved");
    };
    if !d["notes"]
        .as_array()
        .is_some_and(|ns| ns.iter().any(|n| n["resolvable"] == true))
    {
        return bad_request("Discussion is not resolvable");
    }
    let now = now(w);
    let st = svc_mut(w);
    let current = current_username(st);
    let ids: Vec<String> = d["notes"]
        .as_array()
        .map(|ns| ns.iter().map(|n| n["id"].to_string()).collect())
        .unwrap_or_default();
    for id in ids {
        if let Some(n) = st.collection_mut("notes").get_mut(&id)
            && n["resolvable"].as_bool().unwrap_or(false)
        {
            n["resolved"] = json!(resolved);
            n["resolved_by"] = if resolved {
                json!(current)
            } else {
                Value::Null
            };
            n["resolved_at"] = if resolved { json!(now) } else { Value::Null };
        }
    }
    let st = store(w);
    ok(find_discussion(st, &r.base, &key, &did).unwrap_or(d))
}

// ---------------------------------------------------------------------------
// Award emoji

/// `awardable` key for the request: the note (`note:<id>`) or the issue/MR.
fn awardable(st: &Store, r: &Req) -> Result<(String, String), Response> {
    let key = require_noteable(st, r)?;
    match r.path.get("note_id") {
        Some(nid) => {
            let n = find_note(st, &key, nid).ok_or_else(|| not_found("Note"))?;
            if let Some(d) = r.path.get("discussion_id")
                && str_of(&n, "discussion_id") != d
            {
                return Err(not_found("Note"));
            }
            Ok((format!("note:{nid}"), key))
        }
        None => Ok((key.clone(), key)),
    }
}

fn render_award(st: &Store, base: &str, a: &Value) -> Value {
    let target = str_of(a, "awardable");
    let (id, ty) = match target.strip_prefix("note:") {
        Some(n) => (n.parse::<u64>().unwrap_or(0), "Note"),
        None => (
            noteable_doc(st, target)
                .and_then(|d| as_u64(&d["id"]))
                .unwrap_or(0),
            if target.contains('!') {
                "MergeRequest"
            } else {
                "Issue"
            },
        ),
    };
    json!({
        "id": a["id"],
        "name": str_of(a, "name"),
        "user": user_basic(st, base, str_of(a, "user")),
        "created_at": a["created_at"],
        "updated_at": a.get("updated_at").unwrap_or(&a["created_at"]),
        "awardable_id": id,
        "awardable_type": ty,
        "url": null,
    })
}

fn list_awards(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (target, _) = match awardable(st, r) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let items = all(st, "award_emoji")
        .into_iter()
        .filter(|(_, a)| str_of(a, "awardable") == target)
        .map(|(_, a)| render_award(st, &r.base, a))
        .collect();
    paged(r, items)
}

fn get_award(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (target, _) = match awardable(st, r) {
        Ok(t) => t,
        Err(e) => return e,
    };
    match get(st, "award_emoji", r.p("award_id")).filter(|a| str_of(a, "awardable") == target) {
        Some(a) => ok(render_award(st, &r.base, a)),
        None => not_found("Award Emoji"),
    }
}

const EMOJI: &[&str] = &[
    "thumbsup",
    "thumbsdown",
    "tada",
    "rocket",
    "eyes",
    "heart",
    "smile",
    "laughing",
    "confused",
    "white_check_mark",
    "heavy_check_mark",
    "x",
    "100",
    "fire",
    "clap",
    "pray",
    "thinking",
    "warning",
    "bug",
    "star",
    "ok_hand",
    "raised_hands",
    "hourglass",
    "construction",
    "sparkles",
    "+1",
    "-1",
];

fn create_award(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (target, _) = match awardable(st, r) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let Some(name) = r.s("name") else {
        return missing("name");
    };
    let name = match name.trim_matches(':') {
        "+1" => "thumbsup".to_owned(),
        "-1" => "thumbsdown".to_owned(),
        n => n.to_owned(),
    };
    if !EMOJI.contains(&name.as_str())
        && !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
    {
        return not_found("Award Emoji Name is not a valid emoji name");
    }
    let current = current_username(st);
    if all(st, "award_emoji").iter().any(|(_, a)| {
        str_of(a, "awardable") == target
            && str_of(a, "name") == name
            && str_of(a, "user") == current
    }) {
        return not_found("Award Emoji Name has already been taken");
    }
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "award_emoji", 1);
    let doc = json!({"id": id, "name": name, "user": current, "awardable": target, "created_at": now, "updated_at": now});
    st.collection_mut("award_emoji")
        .insert(id.to_string(), doc.clone());
    created(render_award(st, &r.base, &doc))
}

fn delete_award(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (target, _) = match awardable(st, r) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let id = r.p("award_id").to_owned();
    let Some(a) = get(st, "award_emoji", &id).filter(|a| str_of(a, "awardable") == target) else {
        return not_found("Award Emoji");
    };
    if str_of(a, "user") != current_username(st) {
        return forbidden();
    }
    remove_doc(svc_mut(w), "award_emoji", &id);
    no_content()
}
