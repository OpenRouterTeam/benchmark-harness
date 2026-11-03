//! Tasks, comments, Slides, Forms, Chat, Contacts, Custom Search, Apps Script and auth.

use serde_json::{Value, json};

use crate::drive::{FORM, SLIDES, need_file, new_file};
use crate::util::{Ctx, R, USER, arg, arg_bool, arg_i64, arg_json, arg_list, http_error, s};

// ───────────────────────────── Tasks ─────────────────────────────

fn tasks_err(url: &str, status: u16, msg: &str) -> String {
    format!("API error: {}", http_error(status, url, msg, "notFound"))
}

fn need_list(ctx: &Ctx, id: &str) -> Result<Value, String> {
    let id = if id == "@default" {
        ctx.all("task_lists")
            .first()
            .map(|l| s(l, "id").to_owned())
            .unwrap_or_default()
    } else {
        id.to_owned()
    };
    ctx.get("task_lists", &id).ok_or_else(|| {
        tasks_err(
            &format!("https://tasks.googleapis.com/tasks/v1/users/@me/lists/{id}?alt=json"),
            404,
            "Task list not found.",
        )
    })
}

fn need_task(ctx: &Ctx, list: &str, id: &str) -> Result<Value, String> {
    ctx.get("tasks", id)
        .filter(|t| s(t, "taskListId") == list && !t["deleted"].as_bool().unwrap_or(false))
        .ok_or_else(|| {
            tasks_err(
                &format!("https://tasks.googleapis.com/tasks/v1/lists/{list}/tasks/{id}?alt=json"),
                404,
                "Not Found",
            )
        })
}

fn list_self(id: &str) -> String {
    format!("https://www.googleapis.com/tasks/v1/users/@me/lists/{id}")
}

pub fn list_task_lists(ctx: &mut Ctx, _a: &Value) -> R {
    let lists = ctx.all("task_lists");
    if lists.is_empty() {
        return Ok(format!("No task lists found for {USER}."));
    }
    let mut out = format!("Task Lists for {USER}:\n");
    for l in lists {
        out.push_str(&format!(
            "- {} (ID: {})\n  Updated: {}\n",
            s(&l, "title"),
            s(&l, "id"),
            s(&l, "updated")
        ));
    }
    Ok(out)
}

pub fn get_task_list(ctx: &mut Ctx, a: &Value) -> R {
    let l = need_list(ctx, s(a, "task_list_id"))?;
    Ok(format!(
        "Task List Details for {USER}:\n- Title: {}\n- ID: {}\n- Updated: {}\n- Self Link: {}",
        s(&l, "title"),
        s(&l, "id"),
        s(&l, "updated"),
        list_self(s(&l, "id"))
    ))
}

pub fn manage_task_list(ctx: &mut Ctx, a: &Value) -> R {
    let action = s(a, "action");
    if !["create", "update", "delete", "clear_completed"].contains(&action) {
        return Err(format!(
            "Invalid action '{action}'. Must be one of: create, update, delete, clear_completed"
        ));
    }
    let now = ctx.rfc3339_ms();
    match action {
        "create" => {
            let title = arg(a, "title")
                .ok_or_else(|| "'title' is required for the 'create' action.".to_owned())?;
            let id = ctx.st.ids.next_prefixed("task_list", "MTc", 22);
            ctx.put(
                "task_lists",
                json!({"id": id, "title": title, "updated": now}),
            );
            Ok(format!(
                "Task List Created for {USER}:\n- Title: {title}\n- ID: {id}\n- Created: {now}\n- Self Link: {}",
                list_self(&id)
            ))
        }
        _ => {
            let lid = arg(a, "task_list_id")
                .ok_or_else(|| format!("'task_list_id' is required for the '{action}' action."))?
                .to_owned();
            let mut l = need_list(ctx, &lid)?;
            match action {
                "update" => {
                    let title = arg(a, "title")
                        .ok_or_else(|| "'title' is required for the 'update' action.".to_owned())?;
                    l["title"] = json!(title);
                    l["updated"] = json!(now);
                    ctx.put("task_lists", l.clone());
                    Ok(format!(
                        "Task List Updated for {USER}:\n- Title: {title}\n- ID: {lid}\n- Updated: {now}"
                    ))
                }
                "delete" => {
                    ctx.remove("task_lists", s(&l, "id"));
                    for t in ctx
                        .all("tasks")
                        .into_iter()
                        .filter(|t| s(t, "taskListId") == s(&l, "id"))
                    {
                        ctx.remove("tasks", s(&t, "id"));
                    }
                    Ok(format!(
                        "Task list {lid} has been deleted for {USER}. All tasks in this list have also been deleted."
                    ))
                }
                _ => {
                    for mut t in ctx.all("tasks").into_iter().filter(|t| {
                        s(t, "taskListId") == s(&l, "id") && s(t, "status") == "completed"
                    }) {
                        t["hidden"] = json!(true);
                        ctx.put("tasks", t);
                    }
                    Ok(format!(
                        "All completed tasks have been cleared from task list {lid} for {USER}. The tasks are now hidden and won't appear in default task list views."
                    ))
                }
            }
        }
    }
}

fn task_line(t: &Value, indent: &str, bullet: &str) -> String {
    let mut out = format!(
        "{indent}{bullet} {} (ID: {})\n{indent}  Status: {}\n",
        if s(t, "title").is_empty() {
            "Untitled"
        } else {
            s(t, "title")
        },
        s(t, "id"),
        s(t, "status")
    );
    if !s(t, "due").is_empty() {
        out.push_str(&format!("{indent}  Due: {}\n", s(t, "due")));
    }
    let notes = s(t, "notes");
    if !notes.is_empty() {
        let short: String = notes.chars().take(100).collect();
        out.push_str(&format!(
            "{indent}  Notes: {short}{}\n",
            if notes.chars().count() > 100 {
                "..."
            } else {
                ""
            }
        ));
    }
    if !s(t, "completed").is_empty() {
        out.push_str(&format!("{indent}  Completed: {}\n", s(t, "completed")));
    }
    out.push_str(&format!("{indent}  Updated: {}\n\n", s(t, "updated")));
    out
}

pub fn list_tasks(ctx: &mut Ctx, a: &Value) -> R {
    let l = need_list(ctx, s(a, "task_list_id"))?;
    let lid = s(&l, "id").to_owned();
    let show_completed = arg_bool(a, "show_completed").unwrap_or(true);
    let show_hidden = arg_bool(a, "show_hidden").unwrap_or(false);
    let show_deleted = arg_bool(a, "show_deleted").unwrap_or(false);
    let due_min = arg(a, "due_min").map(str::to_owned);
    let due_max = arg(a, "due_max").map(|d| {
        if d.len() == 10 {
            format!("{d}T23:59:59Z")
        } else {
            d.to_owned()
        }
    });
    let mut tasks: Vec<Value> = ctx
        .all("tasks")
        .into_iter()
        .filter(|t| s(t, "taskListId") == lid)
        .filter(|t| show_deleted || !t["deleted"].as_bool().unwrap_or(false))
        .filter(|t| show_hidden || !t["hidden"].as_bool().unwrap_or(false))
        .filter(|t| show_completed || s(t, "status") != "completed")
        .filter(|t| {
            due_min
                .as_ref()
                .is_none_or(|m| !s(t, "due").is_empty() && s(t, "due") >= m.as_str())
        })
        .filter(|t| {
            due_max
                .as_ref()
                .is_none_or(|m| !s(t, "due").is_empty() && s(t, "due") <= m.as_str())
        })
        .collect();
    tasks.sort_by(|x, y| s(x, "position").cmp(s(y, "position")));
    let max = arg_i64(a, "max_results").unwrap_or(20).max(1) as usize;
    tasks.truncate(max);
    if tasks.is_empty() {
        return Ok(format!(
            "No tasks found in task list {} for {USER}.",
            s(a, "task_list_id")
        ));
    }
    let ids: Vec<String> = tasks.iter().map(|t| s(t, "id").to_owned()).collect();
    let mut out = format!("Tasks in list {} for {USER}:\n", s(a, "task_list_id"));
    for t in tasks
        .iter()
        .filter(|t| s(t, "parent").is_empty() || !ids.iter().any(|i| i == s(t, "parent")))
    {
        out.push_str(&task_line(t, "", "-"));
        for c in tasks.iter().filter(|c| s(c, "parent") == s(t, "id")) {
            out.push_str(&task_line(c, "  ", "*"));
        }
    }
    Ok(out)
}

pub fn get_task(ctx: &mut Ctx, a: &Value) -> R {
    let l = need_list(ctx, s(a, "task_list_id"))?;
    let t = need_task(ctx, s(&l, "id"), s(a, "task_id"))?;
    let mut out = format!(
        "Task Details for {USER}:\n- Title: {}\n- ID: {}\n- Status: {}\n- Updated: {}",
        s(&t, "title"),
        s(&t, "id"),
        s(&t, "status"),
        s(&t, "updated")
    );
    for (k, label) in [
        ("due", "Due Date"),
        ("completed", "Completed"),
        ("notes", "Notes"),
        ("parent", "Parent Task ID"),
        ("position", "Position"),
    ] {
        if !s(&t, k).is_empty() {
            out.push_str(&format!("\n- {label}: {}", s(&t, k)));
        }
    }
    out.push_str(&format!(
        "\n- Self Link: https://www.googleapis.com/tasks/v1/lists/{}/tasks/{}",
        s(&l, "id"),
        s(&t, "id")
    ));
    out.push_str(&format!(
        "\n- Web View Link: https://tasks.google.com/task/{}",
        s(&t, "id")
    ));
    Ok(out)
}

fn valid_due(due: &str) -> Result<String, String> {
    let ok = crate::util::parse_datetime(due, Some("UTC")).is_some();
    if !ok {
        return Err(format!(
            "Invalid due date format. Expected RFC 3339 datetime (e.g., '2026-04-25T00:00:00Z'), got '{due}'"
        ));
    }
    // The Tasks API keeps the date only.
    Ok(format!("{}T00:00:00.000Z", &due[..10]))
}

pub fn manage_task(ctx: &mut Ctx, a: &Value) -> R {
    let action = s(a, "action");
    let status = arg(a, "status");
    if status.is_some_and(|x| !["needsAction", "completed"].contains(&x)) {
        return Err("invalid status: must be 'needsAction' or 'completed'".into());
    }
    let due = match arg(a, "due") {
        Some(d) => Some(valid_due(d)?),
        None => None,
    };
    if !["create", "update", "delete", "move"].contains(&action) {
        return Err(format!(
            "Invalid action '{action}'. Must be one of: create, update, delete, move"
        ));
    }
    let l = need_list(ctx, s(a, "task_list_id"))?;
    let lid = s(&l, "id").to_owned();
    let now = ctx.rfc3339_ms();
    if action == "create" {
        if status.is_some() {
            return Err("'status' is only supported for the 'update' action.".into());
        }
        let title = arg(a, "title")
            .ok_or_else(|| "'title' is required for the 'create' action.".to_owned())?;
        let id = ctx.st.ids.next_prefixed("task", "T", 21);
        let min_pos = ctx
            .all("tasks")
            .iter()
            .filter(|t| s(t, "taskListId") == lid)
            .map(|t| s(t, "position").to_owned())
            .min()
            .unwrap_or_else(|| "00000000000000000001".into());
        let pos: u128 = min_pos.parse::<u128>().unwrap_or(1).saturating_sub(1);
        let mut t = json!({"id": id, "taskListId": lid, "title": title, "status": "needsAction", "updated": now, "position": format!("{pos:020}")});
        if let Some(n) = a.get("notes").and_then(Value::as_str) {
            t["notes"] = json!(n);
        }
        if let Some(d) = &due {
            t["due"] = json!(d);
        }
        if let Some(p) = arg(a, "parent") {
            t["parent"] = json!(p);
        }
        ctx.put("tasks", t.clone());
        let mut out = format!(
            "Task Created for {USER}:\n- Title: {title}\n- ID: {id}\n- Status: needsAction\n- Updated: {now}"
        );
        if let Some(d) = &due {
            out.push_str(&format!("\n- Due Date: {d}"));
        }
        if !s(&t, "notes").is_empty() {
            out.push_str(&format!("\n- Notes: {}", s(&t, "notes")));
        }
        out.push_str(&format!(
            "\n- Web View Link: https://tasks.google.com/task/{id}"
        ));
        return Ok(out);
    }
    let tid = arg(a, "task_id")
        .ok_or_else(|| format!("'task_id' is required for the '{action}' action."))?
        .to_owned();
    let mut t = need_task(ctx, &lid, &tid)?;
    match action {
        "update" => {
            if let Some(x) = arg(a, "title") {
                t["title"] = json!(x);
            }
            if let Some(x) = a.get("notes").and_then(Value::as_str) {
                t["notes"] = json!(x);
            }
            if let Some(d) = &due {
                t["due"] = json!(d);
            }
            if let Some(st) = status {
                t["status"] = json!(st);
                if st == "completed" {
                    t["completed"] = json!(now);
                } else if let Some(o) = t.as_object_mut() {
                    o.remove("completed");
                }
            }
            t["updated"] = json!(now);
            ctx.put("tasks", t.clone());
            let mut out = format!(
                "Task Updated for {USER}:\n- Title: {}\n- ID: {tid}\n- Status: {}\n- Updated: {now}",
                s(&t, "title"),
                s(&t, "status")
            );
            for (k, label) in [
                ("due", "Due Date"),
                ("notes", "Notes"),
                ("completed", "Completed"),
            ] {
                if !s(&t, k).is_empty() {
                    out.push_str(&format!("\n- {label}: {}", s(&t, k)));
                }
            }
            Ok(out)
        }
        "delete" => {
            ctx.remove("tasks", &tid);
            Ok(format!(
                "Task {tid} has been deleted from task list {} for {USER}.",
                s(a, "task_list_id")
            ))
        }
        _ => {
            let mut details = Vec::new();
            if let Some(d) = arg(a, "destination_task_list") {
                need_list(ctx, d)?;
                t["taskListId"] = json!(d);
                details.push(format!("moved to task list {d}"));
            }
            if let Some(p) = arg(a, "parent") {
                t["parent"] = json!(p);
                details.push(format!("made a subtask of {p}"));
            }
            if let Some(p) = arg(a, "previous") {
                details.push(format!("positioned after {p}"));
            }
            t["updated"] = json!(now);
            ctx.put("tasks", t.clone());
            let mut out = format!(
                "Task Moved for {USER}:\n- Title: {}\n- ID: {tid}\n- Status: {}\n- Updated: {now}",
                s(&t, "title"),
                s(&t, "status")
            );
            if !s(&t, "parent").is_empty() {
                out.push_str(&format!("\n- Parent Task ID: {}", s(&t, "parent")));
            }
            out.push_str(&format!("\n- Position: {}", s(&t, "position")));
            if !details.is_empty() {
                out.push_str(&format!("\n- Move Details: {}", details.join(", ")));
            }
            Ok(out)
        }
    }
}

// ───────────────────────────── Comments ─────────────────────────────

fn field(label: &str, value: &str) -> String {
    let pad = format!("\n{}", " ".repeat(label.len()));
    format!("{label}{}", value.lines().collect::<Vec<_>>().join(&pad))
}

pub fn list_comments(ctx: &mut Ctx, a: &Value, app: &str, id_param: &str) -> R {
    let fid = s(a, id_param).to_owned();
    need_file(ctx, &format!("list_{app}_comments"), &fid)?;
    let max = arg_i64(a, "max_comments").unwrap_or(100);
    if max < 1 {
        return Err("max_comments must be a positive integer.".into());
    }
    let comments: Vec<Value> = ctx
        .all("drive_comments")
        .into_iter()
        .filter(|c| s(c, "fileId") == fid && !c["deleted"].as_bool().unwrap_or(false))
        .take(max as usize)
        .collect();
    if comments.is_empty() {
        return Ok(format!("No comments found in {app} {fid}"));
    }
    let mut out = vec![format!(
        "Found {} comments in {app} {fid}:\n",
        comments.len()
    )];
    for c in comments {
        out.push(format!("Comment ID: {}", s(&c, "id")));
        out.push(format!("Author: {}", s(&c["author"], "displayName")));
        out.push(format!(
            "Created: {}{}",
            s(&c, "createdTime"),
            if c["resolved"].as_bool().unwrap_or(false) {
                " [RESOLVED]"
            } else {
                ""
            }
        ));
        if !s(&c["quotedFileContent"], "value").is_empty() {
            out.push(field("Quoted text: ", s(&c["quotedFileContent"], "value")));
        }
        if !s(&c, "anchor").is_empty() {
            out.push(field("Anchor: ", s(&c, "anchor")));
        }
        out.push(field("Content: ", s(&c, "content")));
        let replies = c["replies"].as_array().cloned().unwrap_or_default();
        if !replies.is_empty() {
            out.push(format!("  Replies ({}):", replies.len()));
            for r in replies {
                out.push(format!("    Reply ID: {}", s(&r, "id")));
                out.push(format!("    Author: {}", s(&r["author"], "displayName")));
                out.push(format!("    Created: {}", s(&r, "createdTime")));
                out.push(field("    Content: ", s(&r, "content")));
            }
        }
        out.push(String::new());
    }
    Ok(out.join("\n"))
}

fn me() -> Value {
    json!({"displayName": "Ops Team", "emailAddress": USER, "me": true})
}

pub fn manage_comment(ctx: &mut Ctx, a: &Value, app: &str, id_param: &str) -> R {
    let fid = s(a, id_param).to_owned();
    let action = s(a, "action").trim().to_lowercase();
    let content = arg(a, "comment_content").map(str::to_owned);
    let cid = arg(a, "comment_id").map(str::to_owned);
    let cell = arg(a, "cell").map(str::to_owned);
    if cell.is_some() && ["reply", "resolve", "update", "delete"].contains(&action.as_str()) {
        return Err("cell is only supported for the create action".into());
    }
    need_file(ctx, &format!("manage_{app}_comment"), &fid)?;
    let now = ctx.rfc3339_ms();
    let find = |ctx: &Ctx, cid: &str| -> Result<Value, String> {
        ctx.get("drive_comments", cid).filter(|c| s(c, "fileId") == fid && !c["deleted"].as_bool().unwrap_or(false)).ok_or_else(|| {
            format!("API error in manage_{app}_comment: {}", http_error(404, &format!("https://www.googleapis.com/drive/v3/files/{fid}/comments/{cid}?alt=json"), &format!("Comment not found: {cid}."), "notFound"))
        })
    };
    match action.as_str() {
        "create" => {
            let content = content
                .ok_or_else(|| "comment_content is required for create action".to_owned())?;
            let id = ctx.st.ids.next_prefixed("comment", "AAAB", 11);
            let mut c = json!({"id": id, "fileId": fid, "content": content, "author": me(), "createdTime": now, "modifiedTime": now, "resolved": false, "replies": []});
            if let Some(cell) = &cell {
                c["anchor"] = json!(cell);
                ctx.put("drive_comments", c);
                return Ok(format!(
                    "Comment created successfully on {cell}!\nComment ID: {id}\n{}",
                    field("Content: ", &content)
                ));
            }
            ctx.put("drive_comments", c);
            Ok(format!(
                "Comment created successfully!\nComment ID: {id}\nAuthor: Ops Team\nCreated: {now}\n{}",
                field("Content: ", &content)
            ))
        }
        "reply" => {
            let (Some(cid), Some(content)) = (cid, content) else {
                return Err("comment_id and comment_content are required for reply action".into());
            };
            let mut c = find(ctx, &cid)?;
            let rid = ctx.st.ids.next_prefixed("reply", "AAAB", 11);
            let mut replies = c["replies"].as_array().cloned().unwrap_or_default();
            replies.push(json!({"id": rid, "content": content, "author": me(), "createdTime": now, "modifiedTime": now}));
            c["replies"] = json!(replies);
            ctx.put("drive_comments", c);
            Ok(format!(
                "Reply posted successfully!\nReply ID: {rid}\nAuthor: Ops Team\nCreated: {now}\n{}",
                field("Content: ", &content)
            ))
        }
        "resolve" => {
            let cid = cid.ok_or_else(|| "comment_id is required for resolve action".to_owned())?;
            let mut c = find(ctx, &cid)?;
            let rid = ctx.st.ids.next_prefixed("reply", "AAAB", 11);
            let mut replies = c["replies"].as_array().cloned().unwrap_or_default();
            replies.push(json!({"id": rid, "content": "", "action": "resolve", "author": me(), "createdTime": now, "modifiedTime": now}));
            c["replies"] = json!(replies);
            c["resolved"] = json!(true);
            ctx.put("drive_comments", c);
            Ok(format!(
                "Comment {cid} has been resolved successfully.\nResolve reply ID: {rid}\nAuthor: Ops Team\nCreated: {now}"
            ))
        }
        "update" => {
            let (Some(cid), Some(content)) = (cid, content) else {
                return Err("comment_id and comment_content are required for update action".into());
            };
            let mut c = find(ctx, &cid)?;
            if s(&c["author"], "emailAddress") != USER {
                return Err(format!(
                    "Comment {cid} was written by another user; only its author can edit it."
                ));
            }
            c["content"] = json!(content);
            c["modifiedTime"] = json!(now);
            ctx.put("drive_comments", c);
            Ok(format!(
                "Comment {cid} updated successfully!\nAuthor: Ops Team\nModified: {now}\n{}",
                field("Content: ", &content)
            ))
        }
        "delete" => {
            let cid = cid.ok_or_else(|| "comment_id is required for delete action".to_owned())?;
            let mut c = find(ctx, &cid)?;
            if s(&c["author"], "emailAddress") != USER {
                return Err(format!(
                    "Comment {cid} was written by another user; only its author can delete it."
                ));
            }
            c["deleted"] = json!(true);
            ctx.put("drive_comments", c);
            Ok(format!("Comment {cid} deleted from {app} {fid}."))
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'create', 'reply', 'resolve', 'update', or 'delete'."
        )),
    }
}

// ───────────────────────────── Slides ─────────────────────────────

fn need_pres(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    ctx.get("drive_files", id)
        .filter(|f| s(f, "mimeType") == SLIDES)
        .ok_or_else(|| {
            format!(
                "API error in {tool}: {}",
                http_error(
                    404,
                    &format!("https://slides.googleapis.com/v1/presentations/{id}?alt=json"),
                    "Requested entity was not found.",
                    "notFound"
                )
            )
        })
}

pub fn create_presentation(ctx: &mut Ctx, a: &Value) -> R {
    let title = arg(a, "title")
        .unwrap_or("Untitled Presentation")
        .to_owned();
    let mut f = new_file(ctx, &title, SLIDES, None);
    f["slides"] = json!([{"objectId": "p", "elements": []}]);
    let id = s(&f, "id").to_owned();
    ctx.put("drive_files", f);
    Ok(format!(
        "Presentation Created Successfully for {USER}:\n- Title: {title}\n- Presentation ID: {id}\n- URL: https://docs.google.com/presentation/d/{id}/edit\n- Slides: 1 slide(s) created"
    ))
}

fn slide_text(sl: &Value) -> String {
    let rows: Vec<String> = sl["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|e| s(e, "text").lines().map(str::to_owned).collect::<Vec<_>>())
        .filter(|r| !r.trim().is_empty())
        .map(|r| format!("    > {r}"))
        .collect();
    if rows.is_empty() {
        "empty".into()
    } else {
        format!("\n{}", rows.join("\n"))
    }
}

pub fn get_presentation(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "presentation_id").to_owned();
    let f = need_pres(ctx, "get_presentation", &id)?;
    let slides = f["slides"].as_array().cloned().unwrap_or_default();
    if arg_bool(a, "raw").unwrap_or(false) {
        return Ok(serde_json::to_string(
            &json!({"presentationId": id, "title": s(&f, "name"), "slides": slides}),
        )
        .expect("JSON"));
    }
    let mut info = Vec::new();
    for (i, sl) in slides.iter().enumerate() {
        info.push(format!(
            "  Slide {}: ID {}, {} element(s), text: {}",
            i + 1,
            s(sl, "objectId"),
            sl["elements"].as_array().map_or(0, Vec::len),
            slide_text(sl)
        ));
        if arg_bool(a, "include_speaker_notes").unwrap_or(false) {
            info.push(format!(
                "    Speaker notes (shape {}_notes): {}",
                s(sl, "objectId"),
                if s(sl, "notes").is_empty() {
                    "(empty)"
                } else {
                    s(sl, "notes")
                }
            ));
        }
    }
    Ok(format!(
        "Presentation Details for {USER}:\n- Title: {}\n- Presentation ID: {id}\n- URL: https://docs.google.com/presentation/d/{id}/edit\n- Total Slides: {}\n- Page Size: 9144000 x 5143500 EMU\n\nSlides Breakdown:\n{}",
        s(&f, "name"),
        slides.len(),
        if info.is_empty() {
            "  No slides found".to_owned()
        } else {
            info.join("\n")
        }
    ))
}

pub fn batch_update_presentation(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "presentation_id").to_owned();
    let mut f = need_pres(ctx, "batch_update_presentation", &id)?;
    let reqs = match arg_json(a, "requests") {
        Some(Value::Array(r)) => r,
        _ => return Err("requests must be a list of request objects".into()),
    };
    let mut slides = f["slides"].as_array().cloned().unwrap_or_default();
    let mut replies = Vec::new();
    for (i, r) in reqs.iter().enumerate() {
        let Some((kind, body)) = r
            .as_object()
            .and_then(|o| (o.len() == 1).then(|| o.iter().next()).flatten())
        else {
            return Err(format!(
                "Request {i} must contain exactly one Slides request type."
            ));
        };
        match kind.as_str() {
            "createSlide" => {
                let oid = body
                    .get("objectId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("g{}", ctx.hex_id("slide", 12)));
                let at = body
                    .get("insertionIndex")
                    .and_then(Value::as_u64)
                    .map(|x| x as usize)
                    .unwrap_or(slides.len())
                    .min(slides.len());
                slides.insert(at, json!({"objectId": oid, "elements": []}));
                replies.push(json!({"createSlide": {"objectId": oid}}));
            }
            "createShape" => {
                let oid = body
                    .get("objectId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("g{}", ctx.hex_id("shape", 12)));
                let page = body["elementProperties"]["pageObjectId"]
                    .as_str()
                    .unwrap_or("");
                let sl = slides.iter_mut().find(|x| s(x, "objectId") == page).ok_or_else(|| format!("Invalid requests[{i}].createShape: The page ({page}) could not be found."))?;
                let mut els = sl["elements"].as_array().cloned().unwrap_or_default();
                els.push(json!({"objectId": oid, "type": body.get("shapeType").cloned().unwrap_or(json!("TEXT_BOX")), "text": ""}));
                sl["elements"] = json!(els);
                replies.push(json!({"createShape": {"objectId": oid}}));
            }
            "insertText" => {
                let oid = body.get("objectId").and_then(Value::as_str).unwrap_or("");
                let text = body.get("text").and_then(Value::as_str).unwrap_or("");
                let el = slides
                    .iter_mut()
                    .flat_map(|sl| sl["elements"].as_array_mut().into_iter().flatten())
                    .find(|e| s(e, "objectId") == oid);
                match el {
                    Some(e) => {
                        let cur = s(e, "text").to_owned();
                        let at = body
                            .get("insertionIndex")
                            .and_then(Value::as_u64)
                            .unwrap_or(cur.chars().count() as u64)
                            as usize;
                        let b = cur
                            .char_indices()
                            .nth(at)
                            .map(|(b, _)| b)
                            .unwrap_or(cur.len());
                        e["text"] = json!(format!("{}{text}{}", &cur[..b], &cur[b..]));
                    }
                    None => {
                        return Err(format!(
                            "insertText request {i} targets '{oid}', which is not a text-capable shape or table on this presentation. Create a text box with createShape first."
                        ));
                    }
                }
                replies.push(json!({}));
            }
            "deleteText" => {
                let oid = body.get("objectId").and_then(Value::as_str).unwrap_or("");
                for e in slides
                    .iter_mut()
                    .flat_map(|sl| sl["elements"].as_array_mut().into_iter().flatten())
                    .filter(|e| s(e, "objectId") == oid)
                {
                    e["text"] = json!("");
                }
                replies.push(json!({}));
            }
            "replaceAllText" => {
                let find = body["containsText"]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                let rep = body
                    .get("replaceText")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let mut n = 0;
                for e in slides
                    .iter_mut()
                    .flat_map(|sl| sl["elements"].as_array_mut().into_iter().flatten())
                {
                    let t = s(e, "text").to_owned();
                    if !find.is_empty() && t.contains(&find) {
                        n += t.matches(&find).count();
                        e["text"] = json!(t.replace(&find, &rep));
                    }
                }
                replies.push(json!({"replaceAllText": {"occurrencesChanged": n}}));
            }
            "deleteObject" => {
                let oid = body.get("objectId").and_then(Value::as_str).unwrap_or("");
                slides.retain(|sl| s(sl, "objectId") != oid);
                for sl in slides.iter_mut() {
                    if let Some(els) = sl["elements"].as_array_mut() {
                        els.retain(|e| s(e, "objectId") != oid);
                    }
                }
                replies.push(json!({}));
            }
            _ => replies.push(json!({})),
        }
    }
    f["slides"] = json!(slides);
    f["modifiedTime"] = json!(ctx.rfc3339_ms());
    ctx.put("drive_files", f);
    let mut out = format!(
        "Batch Update Completed for {USER}:\n- Presentation ID: {id}\n- URL: https://docs.google.com/presentation/d/{id}/edit\n- Requests Applied: {}\n- Replies Received: {}",
        reqs.len(),
        replies.len()
    );
    if !replies.is_empty() {
        out.push_str("\n\nUpdate Results:");
        for (i, r) in replies.iter().enumerate() {
            if let Some(x) = r.get("createSlide") {
                out.push_str(&format!(
                    "\n  Request {}: Created slide with ID {}",
                    i + 1,
                    s(x, "objectId")
                ));
            } else if let Some(x) = r.get("createShape") {
                out.push_str(&format!(
                    "\n  Request {}: Created shape with ID {}",
                    i + 1,
                    s(x, "objectId")
                ));
            } else {
                out.push_str(&format!("\n  Request {}: Operation completed", i + 1));
            }
        }
    }
    Ok(out)
}

pub fn get_page(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "presentation_id").to_owned();
    let pid = s(a, "page_object_id").to_owned();
    let f = need_pres(ctx, "get_page", &id)?;
    let sl = f["slides"]
        .as_array()
        .and_then(|x| x.iter().find(|sl| s(sl, "objectId") == pid).cloned())
        .ok_or_else(|| {
            format!(
                "API error in get_page: {}",
                http_error(
                    404,
                    &format!(
                        "https://slides.googleapis.com/v1/presentations/{id}/pages/{pid}?alt=json"
                    ),
                    &format!("The object ({pid}) could not be found."),
                    "notFound"
                )
            )
        })?;
    let els = sl["elements"].as_array().cloned().unwrap_or_default();
    if arg_bool(a, "raw").unwrap_or(false) {
        return Ok(serde_json::to_string(&sl).expect("JSON"));
    }
    let info: Vec<String> = els
        .iter()
        .map(|e| {
            format!(
                "  Shape: ID {}, Text: '{}'",
                s(e, "objectId"),
                s(e, "text").trim_end()
            )
        })
        .collect();
    Ok(format!(
        "Page Details for {USER}:\n- Presentation ID: {id}\n- Page ID: {pid}\n- Page Type: SLIDE\n- Total Elements: {}\n\nPage Elements:\n{}",
        els.len(),
        if info.is_empty() {
            "  No elements found".to_owned()
        } else {
            info.join("\n")
        }
    ))
}

pub fn render_page(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "presentation_id").to_owned();
    let pid = s(a, "page_object_id").to_owned();
    let f = need_pres(ctx, "render_page_to_image", &id)?;
    if !f["slides"]
        .as_array()
        .is_some_and(|x| x.iter().any(|sl| s(sl, "objectId") == pid))
    {
        return Err(format!(
            "API error in render_page_to_image: {}",
            http_error(
                404,
                &format!(
                    "https://slides.googleapis.com/v1/presentations/{id}/pages/{pid}/thumbnail?alt=json"
                ),
                &format!("The object ({pid}) could not be found."),
                "notFound"
            )
        ));
    }
    let size = arg(a, "size").unwrap_or("LARGE");
    let (w, h) = match size {
        "SMALL" => (200, 113),
        "MEDIUM" => (800, 450),
        _ => (1600, 900),
    };
    Ok(format!(
        "Rendered page:\n- Presentation ID: {id}\n- Page ID: {pid}\n- Size: {size} ({w}x{h})\n- Content URL: https://lh7-us.googleusercontent.com/slides/{id}/{pid}={w}"
    ))
}

// ───────────────────────────── Forms ─────────────────────────────

fn need_form(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    ctx.get("forms", id).ok_or_else(|| {
        format!(
            "API error in {tool}: {}",
            http_error(
                404,
                &format!("https://forms.googleapis.com/v1/forms/{id}?alt=json"),
                "Requested entity was not found.",
                "notFound"
            )
        )
    })
}

pub fn create_form(ctx: &mut Ctx, a: &Value) -> R {
    let title = s(a, "title").to_owned();
    let f = new_file(ctx, arg(a, "document_title").unwrap_or(&title), FORM, None);
    let id = s(&f, "id").to_owned();
    let rid = format!("1FAIpQL{}", ctx.hex_id("form_responder", 49));
    ctx.put("drive_files", f);
    let form = json!({"id": id, "formId": id, "info": {"title": title, "description": a.get("description").and_then(Value::as_str).unwrap_or(""), "documentTitle": arg(a, "document_title").unwrap_or(&title)}, "items": [], "responderUri": format!("https://docs.google.com/forms/d/e/{rid}/viewform"), "isPublished": true, "isAcceptingResponses": true});
    ctx.put("forms", form.clone());
    Ok(format!(
        "Successfully created form '{title}' for {USER}. Form ID: {id}. Edit URL: https://docs.google.com/forms/d/{id}/edit. Responder URL: {}",
        s(&form, "responderUri")
    ))
}

fn item_type(item: &Value) -> &'static str {
    let q = &item["questionItem"]["question"];
    if q.get("choiceQuestion").is_some() {
        "multiple_choice"
    } else if q.get("textQuestion").is_some() {
        if q["textQuestion"]["paragraph"].as_bool().unwrap_or(false) {
            "paragraph"
        } else {
            "text"
        }
    } else if q.get("scaleQuestion").is_some() {
        "scale"
    } else if q.get("dateQuestion").is_some() {
        "date"
    } else if item.get("pageBreakItem").is_some() {
        "page_break"
    } else if item.get("textItem").is_some() {
        "text_item"
    } else {
        "UNKNOWN"
    }
}

pub fn get_form(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "form_id").to_owned();
    let f = need_form(ctx, "get_form", &id)?;
    let items = f["items"].as_array().cloned().unwrap_or_default();
    let serialized: Vec<Value> = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let mut v = json!({"index": i + 1, "itemId": s(it, "itemId"), "title": s(it, "title"), "type": item_type(it)});
            if it["questionItem"]["question"]["required"].as_bool().unwrap_or(false) {
                v["required"] = json!(true);
            }
            if let Some(opts) = it["questionItem"]["question"]["choiceQuestion"]["options"].as_array() {
                v["options"] = json!(opts.iter().map(|o| s(o, "value")).collect::<Vec<_>>());
            }
            v
        })
        .collect();
    let summary: Vec<String> = serialized
        .iter()
        .map(|x| {
            format!(
                "  {}. {} [{}]{}",
                x["index"],
                s(x, "title"),
                s(x, "type"),
                if x["required"].as_bool().unwrap_or(false) {
                    " (Required)"
                } else {
                    ""
                }
            )
        })
        .collect();
    Ok(format!(
        "Form Details for {USER}:\n- Title: \"{}\"\n- Description: \"{}\"\n- Document Title: \"{}\"\n- Form ID: {id}\n- Edit URL: https://docs.google.com/forms/d/{id}/edit\n- Responder URL: {}\n- Items ({} total):\n{}\n- Items (structured):\n{}",
        s(&f["info"], "title"),
        s(&f["info"], "description"),
        s(&f["info"], "documentTitle"),
        s(&f, "responderUri"),
        items.len(),
        if summary.is_empty() {
            "  No items found".to_owned()
        } else {
            summary.join("\n")
        },
        if serialized.is_empty() {
            "[]".to_owned()
        } else {
            serde_json::to_string_pretty(&serialized).expect("JSON")
        }
    ))
}

pub fn set_publish(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "form_id").to_owned();
    let mut f = need_form(ctx, "set_publish_settings", &id)?;
    let p = arg_bool(a, "is_published").unwrap_or(true);
    let acc = arg_bool(a, "is_accepting_responses").unwrap_or(true);
    f["isPublished"] = json!(p);
    f["isAcceptingResponses"] = json!(acc);
    ctx.put("forms", f);
    let py = |b: bool| if b { "True" } else { "False" };
    Ok(format!(
        "Successfully updated publish settings for form {id} for {USER}. Published: {}, Accepting responses: {}",
        py(p),
        py(acc)
    ))
}

fn answers_text(r: &Value) -> String {
    let lines: Vec<String> = r["answers"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(q, ans)| {
                    let vals: Vec<&str> = ans["textAnswers"]["answers"]
                        .as_array()
                        .map(|x| x.iter().map(|v| s(v, "value")).collect())
                        .unwrap_or_default();
                    if vals.is_empty() {
                        format!("  Question ID {q}: No answer provided")
                    } else {
                        format!("  Question ID {q}: {}", vals.join(", "))
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if lines.is_empty() {
        "  No answers found".into()
    } else {
        lines.join("\n")
    }
}

pub fn get_form_response(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "form_id").to_owned();
    need_form(ctx, "get_form_response", &id)?;
    let rid = s(a, "response_id").to_owned();
    let r = ctx
        .get("form_responses", &rid)
        .filter(|r| s(r, "formId") == id)
        .ok_or_else(|| {
            format!(
                "API error in get_form_response: {}",
                http_error(
                    404,
                    &format!("https://forms.googleapis.com/v1/forms/{id}/responses/{rid}?alt=json"),
                    "Requested entity was not found.",
                    "notFound"
                )
            )
        })?;
    Ok(format!(
        "Form Response Details for {USER}:\n- Form ID: {id}\n- Response ID: {rid}\n- Created: {}\n- Last Submitted: {}\n- Answers:\n{}",
        s(&r, "createTime"),
        s(&r, "lastSubmittedTime"),
        answers_text(&r)
    ))
}

pub fn list_form_responses(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "form_id").to_owned();
    need_form(ctx, "list_form_responses", &id)?;
    let rs: Vec<Value> = ctx
        .all("form_responses")
        .into_iter()
        .filter(|r| s(r, "formId") == id)
        .take(arg_i64(a, "page_size").unwrap_or(10).max(1) as usize)
        .collect();
    if rs.is_empty() {
        return Ok(format!("No responses found for form {id} for {USER}."));
    }
    let lines: Vec<String> = rs
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "  {}. Response ID: {} | Created: {} | Last Submitted: {} | Answers: {}",
                i + 1,
                s(r, "id"),
                s(r, "createTime"),
                s(r, "lastSubmittedTime"),
                r["answers"].as_object().map_or(0, |m| m.len())
            )
        })
        .collect();
    Ok(format!(
        "Form Responses for {USER}:\n- Form ID: {id}\n- Total responses returned: {}\n- Responses:\n{}\nNo more pages.",
        rs.len(),
        lines.join("\n")
    ))
}

pub fn batch_update_form(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "form_id").to_owned();
    let mut f = need_form(ctx, "batch_update_form", &id)?;
    let reqs = match arg_json(a, "requests") {
        Some(Value::Array(r)) if !r.is_empty() => r,
        _ => return Err("requests must be a non-empty list".into()),
    };
    let mut items = f["items"].as_array().cloned().unwrap_or_default();
    let mut results = Vec::new();
    for (i, r) in reqs.iter().enumerate() {
        if let Some(c) = r.get("createItem") {
            let mut item = c["item"].clone();
            let iid = ctx.hex_id("form_item", 8);
            item["itemId"] = json!(iid);
            let at = c["location"]["index"]
                .as_u64()
                .map(|x| x as usize)
                .unwrap_or(items.len())
                .min(items.len());
            items.insert(at, item);
            results.push(format!("  Request {}: Created item {iid}", i + 1));
        } else if let Some(u) = r.get("updateFormInfo") {
            if let Some(t) = u["info"]["title"].as_str() {
                f["info"]["title"] = json!(t);
            }
            if let Some(d) = u["info"]["description"].as_str() {
                f["info"]["description"] = json!(d);
            }
            results.push(format!("  Request {}: Updated form info", i + 1));
        } else if let Some(d) = r.get("deleteItem") {
            let idx = d["location"]["index"].as_u64().unwrap_or(0) as usize;
            if idx < items.len() {
                items.remove(idx);
            }
            results.push(format!("  Request {}: Deleted item at index {idx}", i + 1));
        } else {
            results.push(format!("  Request {}: Operation completed", i + 1));
        }
    }
    f["items"] = json!(items);
    ctx.put("forms", f);
    Ok(format!(
        "Batch Update Completed for {USER}:\n- Form ID: {id}\n- URL: https://docs.google.com/forms/d/{id}/edit\n- Requests Applied: {}\n- Replies Received: {}\n\nUpdate Results:\n{}",
        reqs.len(),
        reqs.len(),
        results.join("\n")
    ))
}

// ───────────────────────────── Chat ─────────────────────────────

pub fn list_spaces(ctx: &mut Ctx, a: &Value) -> R {
    let ty = arg(a, "space_type").unwrap_or("all").to_owned();
    let spaces: Vec<Value> = ctx
        .all("chat_spaces")
        .into_iter()
        .filter(|sp| match ty.as_str() {
            "room" => s(sp, "spaceType") == "SPACE",
            "dm" => s(sp, "spaceType") == "DIRECT_MESSAGE",
            _ => true,
        })
        .collect();
    if spaces.is_empty() {
        return Ok(format!("No Chat spaces found for type '{ty}'."));
    }
    let mut out = vec![format!("Found {} Chat spaces (type: {ty}):", spaces.len())];
    out.extend(spaces.iter().map(|sp| {
        format!(
            "- {} (ID: {}, Type: {})",
            if s(sp, "displayName").is_empty() {
                "Unnamed Space"
            } else {
                s(sp, "displayName")
            },
            s(sp, "id"),
            s(sp, "spaceType")
        )
    }));
    Ok(out.join("\n"))
}

fn need_space(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    ctx.get("chat_spaces", id).ok_or_else(|| {
        format!(
            "API error in {tool}: {}",
            http_error(
                404,
                &format!("https://chat.googleapis.com/v1/{id}?alt=json"),
                "Requested entity was not found.",
                "notFound"
            )
        )
    })
}

pub fn get_chat_messages(ctx: &mut Ctx, a: &Value) -> R {
    let sid = s(a, "space_id").to_owned();
    let sp = need_space(ctx, "get_messages", &sid)?;
    let mut msgs: Vec<Value> = ctx
        .all("chat_messages")
        .into_iter()
        .filter(|m| s(m, "space") == sid)
        .collect();
    msgs.sort_by(|x, y| s(y, "createTime").cmp(s(x, "createTime")));
    if arg(a, "order_by").is_some_and(|o| o.contains("asc")) {
        msgs.reverse();
    }
    if let Some(f) = arg(a, "message_filter") {
        let fl = f.to_lowercase();
        msgs.retain(|m| s(m, "text").to_lowercase().contains(&fl));
    }
    msgs.truncate(arg_i64(a, "page_size").unwrap_or(50).max(1) as usize);
    let name = s(&sp, "displayName");
    if msgs.is_empty() {
        return Ok(format!("No messages found in space '{name}' (ID: {sid})."));
    }
    let mut out = vec![format!("Messages from '{name}' (ID: {sid}):\n")];
    for m in msgs {
        out.push(format!(
            "[{}] {}:",
            s(&m, "createTime"),
            s(&m["sender"], "displayName")
        ));
        out.push(format!("  {}", s(&m, "text")));
        if !s(&m["thread"], "name").is_empty() {
            out.push(format!("  [thread: {}]", s(&m["thread"], "name")));
        }
        out.push(format!("  (Message ID: {})\n", s(&m, "id")));
    }
    Ok(out.join("\n"))
}

pub fn send_chat_message(ctx: &mut Ctx, a: &Value) -> R {
    let sid = s(a, "space_id").to_owned();
    need_space(ctx, "send_message", &sid)?;
    let now = crate::tz::rfc3339(ctx.now).replace('Z', ".000000Z");
    if let Some(name) = arg(a, "message_name") {
        let mut m = ctx.get("chat_messages", name).filter(|m| s(m, "space") == sid).ok_or_else(|| format!("message_name '{name}' does not belong to space '{sid}'. Use a message name returned by get_messages for this space."))?;
        m["text"] = json!(s(a, "message_text"));
        m["lastUpdateTime"] = json!(now);
        ctx.put("chat_messages", m);
        return Ok(format!(
            "Message updated in space '{sid}' by {USER}. Message ID: {name}, Time: {now}"
        ));
    }
    let mid = format!("{sid}/messages/{}", ctx.hex_id("chat_message", 11));
    let thread = match (arg(a, "thread_name"), arg(a, "thread_key")) {
        (Some(t), _) => t.to_owned(),
        (None, Some(k)) => format!("{sid}/threads/{k}"),
        _ => format!("{sid}/threads/{}", ctx.hex_id("chat_thread", 11)),
    };
    ctx.put("chat_messages", json!({"id": mid, "space": sid, "text": s(a, "message_text"), "sender": {"displayName": "Ops Team", "email": USER}, "createTime": now, "thread": {"name": thread}}));
    Ok(format!(
        "Message sent to space '{sid}' by {USER}. Message ID: {mid}, Time: {now}"
    ))
}

pub fn search_chat(ctx: &mut Ctx, a: &Value) -> R {
    let q = s(a, "query").to_owned();
    let ql = q.to_lowercase();
    let sid = arg(a, "space_id").map(str::to_owned);
    let spaces = ctx.all("chat_spaces");
    let msgs: Vec<Value> = ctx
        .all("chat_messages")
        .into_iter()
        .filter(|m| {
            sid.as_ref().is_none_or(|x| s(m, "space") == x)
                && s(m, "text").to_lowercase().contains(&ql)
        })
        .take(arg_i64(a, "page_size").unwrap_or(25).max(1) as usize)
        .collect();
    let context = match &sid {
        Some(x) => format!("space '{x}'"),
        None => format!("the first {} accessible spaces", spaces.len()),
    };
    if msgs.is_empty() {
        return Ok(format!("No messages found matching '{q}' in {context}."));
    }
    let mut out = vec![format!(
        "Found {} messages matching '{q}' in {context}:",
        msgs.len()
    )];
    for m in msgs {
        let sname = spaces
            .iter()
            .find(|sp| s(sp, "id") == s(&m, "space"))
            .map(|sp| s(sp, "displayName").to_owned())
            .unwrap_or_default();
        out.push(format!(
            "- [{}] {} in '{sname}': {}",
            s(&m, "createTime"),
            s(&m["sender"], "displayName"),
            s(&m, "text")
        ));
    }
    Ok(out.join("\n"))
}

pub fn create_reaction(ctx: &mut Ctx, a: &Value) -> R {
    let mid = s(a, "message_id").to_owned();
    let mut m = ctx.get("chat_messages", &mid).ok_or_else(|| {
        format!(
            "API error in create_reaction: {}",
            http_error(
                404,
                &format!("https://chat.googleapis.com/v1/{mid}/reactions?alt=json"),
                "Requested entity was not found.",
                "notFound"
            )
        )
    })?;
    let emoji = s(a, "emoji_unicode").to_owned();
    let rid = format!("{mid}/reactions/{}", ctx.hex_id("chat_reaction", 12));
    let mut rs = m["reactions"].as_array().cloned().unwrap_or_default();
    rs.push(json!({"name": rid, "emoji": emoji, "user": USER}));
    m["reactions"] = json!(rs);
    ctx.put("chat_messages", m);
    Ok(format!(
        "Reacted with {emoji} on message {mid}. Reaction ID: {rid}"
    ))
}

pub fn chat_attachment(ctx: &mut Ctx, a: &Value) -> R {
    let mid = s(a, "message_id").to_owned();
    let m = ctx.get("chat_messages", &mid).ok_or_else(|| {
        format!(
            "API error in download_chat_attachment: {}",
            http_error(
                404,
                &format!("https://chat.googleapis.com/v1/{mid}?alt=json"),
                "Requested entity was not found.",
                "notFound"
            )
        )
    })?;
    let atts = m["attachments"].as_array().cloned().unwrap_or_default();
    if atts.is_empty() {
        return Ok(format!("No attachments found on message {mid}."));
    }
    let idx = arg_i64(a, "attachment_index").unwrap_or(0);
    let Some(att) = usize::try_from(idx).ok().and_then(|i| atts.get(i)) else {
        return Ok(format!(
            "Invalid attachment_index {idx}. Message has {} attachment(s) (0-{}).",
            atts.len(),
            atts.len() - 1
        ));
    };
    let size = s(att, "content").len();
    Ok(format!(
        "Attachment downloaded: {}\nType: {}\nSize: {:.1} KB ({size} bytes)\n\nSaved to: /home/app/.workspace-mcp/attachments/{}",
        s(att, "contentName"),
        s(att, "contentType"),
        size as f64 / 1024.0,
        s(att, "contentName")
    ))
}

// ───────────────────────────── Contacts ─────────────────────────────

fn typed(v: &Value) -> String {
    let label = if !s(v, "formattedType").is_empty() {
        s(v, "formattedType")
    } else {
        s(v, "type")
    };
    if label.is_empty() {
        s(v, "value").to_owned()
    } else {
        format!("{} ({label})", s(v, "value"))
    }
}

fn format_contact(p: &Value, detailed: bool) -> String {
    let mut lines = vec![format!("Contact ID: {}", s(p, "id"))];
    if let Some(n) = p["names"].as_array().and_then(|n| n.first())
        && !s(n, "displayName").is_empty()
    {
        lines.push(format!("Name: {}", s(n, "displayName")));
    }
    for (key, one, many) in [
        ("emailAddresses", "Email", "Emails"),
        ("phoneNumbers", "Phone", "Phones"),
    ] {
        let vals: Vec<Value> = p[key]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !s(e, "value").is_empty())
            .collect();
        match vals.len() {
            0 => {}
            1 => lines.push(format!("{one}: {}", typed(&vals[0]))),
            _ => {
                lines.push(format!("{many}:"));
                lines.extend(vals.iter().map(|v| format!("  - {}", typed(v))));
            }
        }
    }
    if let Some(o) = p["organizations"].as_array().and_then(|o| o.first()) {
        let mut parts = Vec::new();
        if !s(o, "title").is_empty() {
            parts.push(s(o, "title").to_owned());
        }
        if !s(o, "name").is_empty() {
            parts.push(format!("at {}", s(o, "name")));
        }
        if !parts.is_empty() {
            lines.push(format!("Organization: {}", parts.join(" ")));
        }
    }
    if detailed {
        if let Some(a) = p["addresses"].as_array().and_then(|x| x.first()) {
            lines.push(format!("Address: {}", s(a, "formattedValue")));
        }
        if let Some(b) = p["biographies"].as_array().and_then(|x| x.first()) {
            lines.push(format!("Notes: {}", s(b, "value")));
        }
    }
    lines.join("\n")
}

fn contact_text(p: &Value) -> String {
    format!(
        "{} {} {} {}",
        p["names"], p["emailAddresses"], p["phoneNumbers"], p["organizations"]
    )
    .to_lowercase()
}

pub fn list_contacts(ctx: &mut Ctx, a: &Value) -> R {
    let mut cs = ctx.all("contacts");
    if arg(a, "sort_order") == Some("LAST_MODIFIED_DESCENDING") {
        cs.reverse();
    } else {
        cs.sort_by_key(|c| s(&c["names"][0], "displayName").to_lowercase());
    }
    cs.truncate(arg_i64(a, "page_size").unwrap_or(100).max(1) as usize);
    if cs.is_empty() {
        return Ok(format!("No contacts found for {USER}."));
    }
    let mut out = format!("Contacts for {USER} ({} found):\n\n", cs.len());
    for c in cs {
        out.push_str(&format_contact(&c, false));
        out.push_str("\n\n");
    }
    Ok(out)
}

fn need_contact(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    let id = id.trim_start_matches("people/");
    ctx.get("contacts", id).ok_or_else(|| {
        format!(
            "API error in {tool}: {}",
            http_error(
                404,
                &format!(
                    "https://people.googleapis.com/v1/people/{id}?personFields=names&alt=json"
                ),
                "Requested entity was not found.",
                "notFound"
            )
        )
    })
}

pub fn get_contact(ctx: &mut Ctx, a: &Value) -> R {
    let c = need_contact(ctx, "get_contact", s(a, "contact_id"))?;
    Ok(format!(
        "Contact Details for {USER}:\n\n{}",
        format_contact(&c, true)
    ))
}

pub fn search_contacts(ctx: &mut Ctx, a: &Value) -> R {
    let q = s(a, "query").to_owned();
    let ql = q.to_lowercase();
    let cs: Vec<Value> = ctx
        .all("contacts")
        .into_iter()
        .filter(|c| contact_text(c).contains(&ql))
        .take(arg_i64(a, "page_size").unwrap_or(30).max(1) as usize)
        .collect();
    if cs.is_empty() {
        return Ok(format!("No contacts found matching '{q}' for {USER}."));
    }
    let mut out = format!("Search Results for '{q}' ({} found):\n\n", cs.len());
    for c in cs {
        out.push_str(&format_contact(&c, false));
        out.push_str("\n\n");
    }
    Ok(out)
}

fn apply_contact(c: &mut Value, a: &Value) {
    let given = arg(a, "given_name")
        .map(str::to_owned)
        .or_else(|| c["names"][0]["givenName"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let family = arg(a, "family_name")
        .map(str::to_owned)
        .or_else(|| c["names"][0]["familyName"].as_str().map(str::to_owned))
        .unwrap_or_default();
    if arg(a, "given_name").is_some() || arg(a, "family_name").is_some() {
        c["names"] = json!([{"givenName": given, "familyName": family, "displayName": format!("{given} {family}").trim()}]);
    }
    let push = |c: &mut Value, key: &str, vals: Vec<Value>| {
        if !vals.is_empty() {
            c[key] = json!(vals);
        }
    };
    let mut emails: Vec<Value> = arg_json(a, "emails")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    if let Some(e) = arg(a, "email") {
        emails.push(json!({"value": e, "type": "work"}));
    }
    push(c, "emailAddresses", emails);
    let mut phones: Vec<Value> = arg_json(a, "phones")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    if let Some(p) = arg(a, "phone") {
        phones.push(json!({"value": p, "type": "mobile"}));
    }
    push(c, "phoneNumbers", phones);
    let mut orgs: Vec<Value> = arg_json(a, "organizations")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    if arg(a, "organization").is_some() || arg(a, "job_title").is_some() {
        orgs.push(json!({"name": arg(a, "organization").unwrap_or(""), "title": arg(a, "job_title").unwrap_or("")}));
    }
    push(c, "organizations", orgs);
    if let Some(n) = arg(a, "notes") {
        c["biographies"] = json!([{"value": n, "contentType": "TEXT_PLAIN"}]);
    }
    if let Some(ad) = arg(a, "address") {
        c["addresses"] = json!([{"formattedValue": ad}]);
    }
}

pub fn manage_contact(ctx: &mut Ctx, a: &Value) -> R {
    let action = s(a, "action");
    match action {
        "create" => {
            let id = format!(
                "c{}",
                ctx.st.ids.next_int("contact", 7_200_000_000_000_000_001)
            );
            let mut c = json!({"id": id, "resourceName": format!("people/{id}")});
            apply_contact(&mut c, a);
            ctx.put("contacts", c.clone());
            Ok(format!(
                "Contact Created for {USER}:\n\n{}",
                format_contact(&c, true)
            ))
        }
        "update" | "delete" => {
            let cid = arg(a, "contact_id")
                .ok_or_else(|| format!("contact_id is required for '{action}' action."))?;
            let mut c = need_contact(ctx, "manage_contact", cid)?;
            if action == "delete" {
                ctx.remove("contacts", s(&c, "id"));
                return Ok(format!("Contact {cid} has been deleted for {USER}."));
            }
            apply_contact(&mut c, a);
            ctx.put("contacts", c.clone());
            Ok(format!(
                "Contact Updated for {USER}:\n\n{}",
                format_contact(&c, true)
            ))
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'create', 'update', or 'delete'."
        )),
    }
}

pub fn list_contact_groups(ctx: &mut Ctx, _a: &Value) -> R {
    let gs = ctx.all("contact_groups");
    if gs.is_empty() {
        return Ok(format!("No contact groups found for {USER}."));
    }
    let mut out = format!("Contact Groups for {USER}:\n\n");
    for g in gs {
        out.push_str(&format!(
            "- {}\n  ID: {}\n  Type: {}\n  Members: {}\n\n",
            s(&g, "name"),
            s(&g, "id"),
            s(&g, "groupType"),
            g["members"].as_array().map_or(0, Vec::len)
        ));
    }
    Ok(out)
}

fn need_group(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    let id = id.trim_start_matches("contactGroups/");
    ctx.get("contact_groups", id).ok_or_else(|| {
        format!(
            "API error in {tool}: {}",
            http_error(
                404,
                &format!("https://people.googleapis.com/v1/contactGroups/{id}?alt=json"),
                "Requested entity was not found.",
                "notFound"
            )
        )
    })
}

pub fn get_contact_group(ctx: &mut Ctx, a: &Value) -> R {
    let g = need_group(ctx, "get_contact_group", s(a, "group_id"))?;
    let members: Vec<String> = g["members"]
        .as_array()
        .map(|m| {
            m.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let mut out = format!(
        "Contact Group Details for {USER}:\n\nName: {}\nID: {}\nType: {}\nTotal Members: {}\n",
        s(&g, "name"),
        s(&g, "id"),
        s(&g, "groupType"),
        members.len()
    );
    if !members.is_empty() {
        let shown: Vec<&String> = members
            .iter()
            .take(arg_i64(a, "max_members").unwrap_or(100).max(1) as usize)
            .collect();
        out.push_str(&format!("\nMembers ({} shown):\n", shown.len()));
        for m in shown {
            out.push_str(&format!("  - {m}\n"));
        }
    }
    Ok(out)
}

pub fn manage_contacts_batch(ctx: &mut Ctx, a: &Value) -> R {
    match s(a, "action") {
        "create" => {
            let list = arg_json(a, "contacts")
                .and_then(|v| v.as_array().cloned())
                .ok_or_else(|| "contacts is required for 'create' action.".to_owned())?;
            let mut out = format!(
                "Batch Create Results for {USER}:\n\nCreated {} contacts:\n\n",
                list.len()
            );
            for spec in list {
                let id = format!(
                    "c{}",
                    ctx.st.ids.next_int("contact", 7_200_000_000_000_000_001)
                );
                let mut c = json!({"id": id, "resourceName": format!("people/{id}")});
                apply_contact(&mut c, &spec);
                out.push_str(&format_contact(&c, false));
                out.push_str("\n\n");
                ctx.put("contacts", c);
            }
            Ok(out)
        }
        "update" => {
            let list = arg_json(a, "updates")
                .and_then(|v| v.as_array().cloned())
                .ok_or_else(|| "updates is required for 'update' action.".to_owned())?;
            let mut out = format!(
                "Batch Update Results for {USER}:\n\nUpdated {} contacts:\n\n",
                list.len()
            );
            for spec in list {
                let mut c = need_contact(ctx, "manage_contacts_batch", s(&spec, "contact_id"))?;
                apply_contact(&mut c, &spec);
                out.push_str(&format_contact(&c, false));
                out.push_str("\n\n");
                ctx.put("contacts", c);
            }
            Ok(out)
        }
        "delete" => {
            let ids = arg_list(a, "contact_ids")
                .ok_or_else(|| "contact_ids is required for 'delete' action.".to_owned())?;
            for id in &ids {
                ctx.remove("contacts", id.trim_start_matches("people/"));
            }
            Ok(format!("Batch deleted {} contacts for {USER}.", ids.len()))
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'create', 'update', or 'delete'."
        )),
    }
}

pub fn manage_contact_group(ctx: &mut Ctx, a: &Value) -> R {
    let action = s(a, "action");
    match action {
        "create" => {
            let name =
                arg(a, "name").ok_or_else(|| "name is required for 'create' action.".to_owned())?;
            let id = ctx.hex_id("contact_group", 15);
            ctx.put(
                "contact_groups",
                json!({"id": id, "name": name, "groupType": "USER_CONTACT_GROUP", "members": []}),
            );
            Ok(format!(
                "Contact Group Created for {USER}:\n\nName: {name}\nID: {id}\nType: USER_CONTACT_GROUP\n"
            ))
        }
        "update" | "delete" | "modify_members" => {
            let gid = arg(a, "group_id")
                .ok_or_else(|| format!("group_id is required for '{action}' action."))?;
            let mut g = need_group(ctx, "manage_contact_group", gid)?;
            match action {
                "update" => {
                    let name = arg(a, "name")
                        .ok_or_else(|| "name is required for 'update' action.".to_owned())?;
                    g["name"] = json!(name);
                    ctx.put("contact_groups", g.clone());
                    Ok(format!(
                        "Contact Group Updated for {USER}:\n\nName: {name}\nID: {}\n",
                        s(&g, "id")
                    ))
                }
                "delete" => {
                    ctx.remove("contact_groups", s(&g, "id"));
                    Ok(format!("Contact group {gid} has been deleted for {USER}."))
                }
                _ => {
                    let mut members: Vec<String> = g["members"]
                        .as_array()
                        .map(|m| {
                            m.iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    let add = arg_list(a, "add_contact_ids").unwrap_or_default();
                    let rm = arg_list(a, "remove_contact_ids").unwrap_or_default();
                    for x in &add {
                        let x = x.trim_start_matches("people/").to_owned();
                        if !members.contains(&x) {
                            members.push(x);
                        }
                    }
                    members.retain(|m| !rm.iter().any(|r| r.trim_start_matches("people/") == m));
                    g["members"] = json!(members);
                    ctx.put("contact_groups", g);
                    Ok(format!(
                        "Contact group {gid} members modified for {USER}:\n\nAdded: {}\nRemoved: {}\n",
                        add.len(),
                        rm.len()
                    ))
                }
            }
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'create', 'update', 'delete', or 'modify_members'."
        )),
    }
}

// ───────────────────────────── Custom Search ─────────────────────────────

pub fn search_custom(ctx: &mut Ctx, a: &Value) -> R {
    let q = s(a, "q").to_owned();
    let ql = q.to_lowercase();
    let words: Vec<&str> = ql.split_whitespace().collect();
    let results: Vec<Value> = ctx
        .all("search_results")
        .into_iter()
        .filter(|r| {
            let t = format!("{} {}", s(r, "title"), s(r, "snippet")).to_lowercase();
            words.iter().all(|w| t.contains(w))
        })
        .collect();
    let start = arg_i64(a, "start").unwrap_or(1).max(1) as usize;
    let num = arg_i64(a, "num").unwrap_or(10).clamp(1, 10) as usize;
    let page: Vec<&Value> = results.iter().skip(start - 1).take(num).collect();
    let mut out = format!(
        "Search Results for {USER}:\n- Query: \"{q}\"\n- Search Engine ID: 017576662512468239146:omuauf_lfve\n- Total Results: {}\n- Search Time: 0.21 seconds\n- Results Returned: {} (showing {} to {})\n\n",
        results.len(),
        page.len(),
        start,
        start + page.len().saturating_sub(1)
    );
    if page.is_empty() {
        out.push_str("No results found.");
        return Ok(out);
    }
    for (i, r) in page.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. {}\n   URL: {}\n   Snippet: {}\n",
            start + i,
            s(r, "title"),
            s(r, "link"),
            s(r, "snippet")
        ));
    }
    if results.len() > start - 1 + num {
        out.push_str(&format!(
            "\n\nTo see more results, search again with start={}",
            start + num
        ));
    }
    Ok(out)
}

pub fn search_engine_info(ctx: &mut Ctx, _a: &Value) -> R {
    Ok(format!(
        "Search Engine Information for {USER}:\n- Search Engine ID: 017576662512468239146:omuauf_lfve\n- Title: Octo Corp Search\n\nSearch Statistics:\n  - Total indexed results: {}\n",
        ctx.all("search_results").len()
    ))
}

// ───────────────────────────── Apps Script ─────────────────────────────

fn need_script(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    ctx.get("scripts", id).ok_or_else(|| {
        format!(
            "API error in {tool}: {}",
            http_error(
                404,
                &format!("https://script.googleapis.com/v1/projects/{id}?alt=json"),
                "Requested entity was not found.",
                "notFound"
            )
        )
    })
}

pub fn get_script_project(ctx: &mut Ctx, a: &Value) -> R {
    match arg(a, "action").unwrap_or("get") {
        "list" => {
            let ps = ctx.all("scripts");
            if ps.is_empty() {
                return Ok("No Apps Script projects found.".into());
            }
            let mut out = vec![format!("Found {} Apps Script projects:", ps.len())];
            out.extend(ps.iter().map(|p| {
                format!(
                    "- {} (ID: {}) Created: {} Modified: {}",
                    s(p, "title"),
                    s(p, "id"),
                    s(p, "createTime"),
                    s(p, "updateTime")
                )
            }));
            Ok(out.join("\n"))
        }
        "get" => {
            let p = need_script(ctx, "get_script_project", s(a, "script_id"))?;
            let files = p["files"].as_array().cloned().unwrap_or_default();
            if let Some(name) = arg(a, "file_name") {
                let Some(f) = files.iter().find(|f| s(f, "name") == name) else {
                    return Ok(format!(
                        "File '{name}' not found in project {}",
                        s(&p, "id")
                    ));
                };
                return Ok(format!(
                    "File: {name} ({})\n\n{}",
                    s(f, "type"),
                    s(f, "source")
                ));
            }
            let mut out = vec![
                format!("Project: {} (ID: {})", s(&p, "title"), s(&p, "id")),
                format!("Creator: {USER}"),
                format!("Created: {}", s(&p, "createTime")),
                format!("Modified: {}", s(&p, "updateTime")),
                String::new(),
                "Files:".into(),
            ];
            for (i, f) in files.iter().enumerate() {
                out.push(format!("{}. {} ({})", i + 1, s(f, "name"), s(f, "type")));
                let src = s(f, "source");
                let short: String = src.chars().take(200).collect();
                out.push(format!(
                    "   {short}{}",
                    if src.chars().count() > 200 { "..." } else { "" }
                ));
            }
            Ok(out.join("\n"))
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'list' or 'get'."
        )),
    }
}

pub fn manage_script_content(ctx: &mut Ctx, a: &Value) -> R {
    let sid = s(a, "script_id").to_owned();
    let mut p = need_script(ctx, "manage_script_content", &sid)?;
    let files = arg_json(a, "files")
        .and_then(|v| v.as_array().cloned())
        .ok_or_else(|| "files must be a non-empty list.".to_owned())?;
    for (i, f) in files.iter().enumerate() {
        if s(f, "name").is_empty() {
            return Err(format!("File at index {i} is missing a non-empty 'name'."));
        }
    }
    let merge = arg_bool(a, "merge").unwrap_or(true);
    let mut existing = if merge {
        p["files"].as_array().cloned().unwrap_or_default()
    } else {
        Vec::new()
    };
    for f in &files {
        existing.retain(|e| s(e, "name") != s(f, "name"));
        existing.push(f.clone());
    }
    p["files"] = json!(existing);
    p["updateTime"] = json!(ctx.rfc3339_ms());
    ctx.put("scripts", p);
    let mut out = vec![
        format!(
            "Updated script project: {sid} ({})",
            if merge { "merged" } else { "replaced" }
        ),
        String::new(),
        "Modified files:".into(),
    ];
    out.extend(
        files
            .iter()
            .map(|f| format!("- {} ({})", s(f, "name"), s(f, "type"))),
    );
    Ok(out.join("\n"))
}

pub fn run_script(ctx: &mut Ctx, a: &Value) -> R {
    let sid = s(a, "script_id").to_owned();
    let p = need_script(ctx, "run_script_function", &sid)?;
    let func = s(a, "function_name").to_owned();
    let found = p["files"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|f| s(f, "source").contains(&format!("function {func}(")));
    if !found {
        return Ok(format!(
            "Execution failed\nFunction: {func}\nError: Script function not found: {func}"
        ));
    }
    Ok(format!(
        "Execution successful\nFunction: {func}\nResult: None"
    ))
}

pub fn manage_deployment(ctx: &mut Ctx, a: &Value) -> R {
    let sid = s(a, "script_id").to_owned();
    let mut p = need_script(ctx, "manage_deployment", &sid)?;
    let mut deps = p["deployments"].as_array().cloned().unwrap_or_default();
    let out = match s(a, "action") {
        "create" => {
            let did = format!("AKfycb{}", ctx.hex_id("deployment", 30));
            let ver = p["versions"].as_array().map_or(0, Vec::len) as i64 + 1;
            let desc = arg(a, "description").unwrap_or("").to_owned();
            deps.push(json!({"deploymentId": did, "versionNumber": ver, "description": desc, "updateTime": ctx.rfc3339_ms()}));
            format!(
                "Created deployment for script: {sid}\nDeployment ID: {did}\nVersion: {ver}\nDescription: {desc}"
            )
        }
        "update" => {
            let did = arg(a, "deployment_id")
                .ok_or_else(|| "deployment_id is required for update.".to_owned())?;
            let d = deps
                .iter_mut()
                .find(|d| s(d, "deploymentId") == did)
                .ok_or_else(|| format!("Deployment {did} not found."))?;
            if let Some(x) = arg(a, "description") {
                d["description"] = json!(x);
            }
            format!(
                "Updated deployment: {did}\nScript: {sid}\nVersion: unchanged\nDescription: {}",
                s(d, "description")
            )
        }
        "delete" => {
            let did = arg(a, "deployment_id")
                .ok_or_else(|| "deployment_id is required for delete.".to_owned())?
                .to_owned();
            deps.retain(|d| s(d, "deploymentId") != did);
            format!("Deleted deployment: {did} from script: {sid}")
        }
        other => {
            return Err(format!(
                "Invalid action '{other}'. Must be 'create', 'update', or 'delete'."
            ));
        }
    };
    p["deployments"] = json!(deps);
    ctx.put("scripts", p);
    Ok(out)
}

pub fn list_deployments(ctx: &mut Ctx, a: &Value) -> R {
    let sid = s(a, "script_id").to_owned();
    let p = need_script(ctx, "list_script_deployments", &sid)?;
    let deps = p["deployments"].as_array().cloned().unwrap_or_default();
    if deps.is_empty() {
        return Ok(format!("No deployments found for script: {sid}"));
    }
    let mut out = vec![format!("Deployments for script: {sid}"), String::new()];
    for (i, d) in deps.iter().enumerate() {
        out.push(format!(
            "{}. {} ({})",
            i + 1,
            if s(d, "description").is_empty() {
                "No description"
            } else {
                s(d, "description")
            },
            s(d, "deploymentId")
        ));
        out.push(format!("   Version: {}", d["versionNumber"]));
        out.push(format!("   Updated: {}", s(d, "updateTime")));
        out.push(String::new());
    }
    Ok(out.join("\n"))
}

pub fn manage_script_project(ctx: &mut Ctx, a: &Value) -> R {
    match s(a, "action") {
        "create" => {
            let title =
                arg(a, "title").ok_or_else(|| "title is required for create.".to_owned())?;
            let id = format!("1{}", ctx.hex_id("script", 56));
            let now = ctx.rfc3339_ms();
            ctx.put("scripts", json!({"id": id, "title": title, "parentId": arg(a, "parent_id"), "createTime": now, "updateTime": now, "files": [{"name": "appsscript", "type": "JSON", "source": "{\"timeZone\":\"America/New_York\",\"exceptionLogging\":\"STACKDRIVER\",\"runtimeVersion\":\"V8\"}"}]}));
            Ok(format!(
                "Created Apps Script project: {title}\nScript ID: {id}\nEdit URL: https://script.google.com/d/{id}/edit"
            ))
        }
        "delete" => {
            let sid = s(a, "script_id").to_owned();
            need_script(ctx, "manage_script_project", &sid)?;
            ctx.remove("scripts", &sid);
            Ok(format!("Deleted Apps Script project: {sid}"))
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'create' or 'delete'."
        )),
    }
}

pub fn manage_script_version(ctx: &mut Ctx, a: &Value) -> R {
    let sid = s(a, "script_id").to_owned();
    let mut p = need_script(ctx, "manage_script_version", &sid)?;
    let mut vers = p["versions"].as_array().cloned().unwrap_or_default();
    match s(a, "action") {
        "create" => {
            let n = vers.len() + 1;
            let desc = arg(a, "description").unwrap_or("").to_owned();
            let now = ctx.rfc3339_ms();
            vers.push(json!({"versionNumber": n, "description": desc, "createTime": now}));
            p["versions"] = json!(vers);
            ctx.put("scripts", p);
            Ok(format!(
                "Created version {n} for script: {sid}\nDescription: {desc}\nCreated: {now}"
            ))
        }
        "list" => {
            if vers.is_empty() {
                return Ok(format!("No versions found for script: {sid}"));
            }
            let mut out = vec![format!("Versions for script: {sid}"), String::new()];
            out.extend(vers.iter().map(|v| {
                format!(
                    "Version {}: {}\n   Created: {}",
                    v["versionNumber"],
                    s(v, "description"),
                    s(v, "createTime")
                )
            }));
            Ok(out.join("\n"))
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'create' or 'list'."
        )),
    }
}

pub fn get_script_version(ctx: &mut Ctx, a: &Value) -> R {
    let sid = s(a, "script_id").to_owned();
    let p = need_script(ctx, "get_script_version", &sid)?;
    let vers = p["versions"].as_array().cloned().unwrap_or_default();
    if arg(a, "action") == Some("list") {
        return manage_script_version(ctx, &json!({"action": "list", "script_id": sid}));
    }
    let n = arg_i64(a, "version_number").unwrap_or(0);
    let v = vers
        .iter()
        .find(|v| v["versionNumber"].as_i64() == Some(n))
        .ok_or_else(|| {
            format!(
                "API error in get_script_version: {}",
                http_error(
                    404,
                    &format!(
                        "https://script.googleapis.com/v1/projects/{sid}/versions/{n}?alt=json"
                    ),
                    "Requested entity was not found.",
                    "notFound"
                )
            )
        })?;
    Ok(format!(
        "Version {n} of script: {sid}\nDescription: {}\nCreated: {}",
        s(v, "description"),
        s(v, "createTime")
    ))
}

pub fn get_script_activity(ctx: &mut Ctx, a: &Value) -> R {
    match arg(a, "action").unwrap_or("processes") {
        "metrics" => {
            let sid = s(a, "script_id").to_owned();
            need_script(ctx, "get_script_activity", &sid)?;
            Ok(format!(
                "Metrics for script: {sid}\nGranularity: {}\n\nNo metrics data available.",
                arg(a, "metrics_granularity").unwrap_or("DAILY")
            ))
        }
        _ => Ok("No recent script executions found.".into()),
    }
}

pub fn trigger_code(_ctx: &mut Ctx, a: &Value) -> R {
    let ty = s(a, "trigger_type");
    let func = s(a, "function_name");
    let schedule = arg(a, "schedule").unwrap_or("");
    let builder = match ty {
        "time_minutes" => format!(
            "ScriptApp.newTrigger('{func}')\n    .timeBased()\n    .everyMinutes({})\n    .create();",
            if schedule.is_empty() { "5" } else { schedule }
        ),
        "time_hours" => format!(
            "ScriptApp.newTrigger('{func}')\n    .timeBased()\n    .everyHours({})\n    .create();",
            if schedule.is_empty() { "1" } else { schedule }
        ),
        "time_daily" => format!(
            "ScriptApp.newTrigger('{func}')\n    .timeBased()\n    .everyDays(1)\n    .atHour({})\n    .create();",
            if schedule.is_empty() { "9" } else { schedule }
        ),
        "on_open" => format!(
            "ScriptApp.newTrigger('{func}')\n    .forSpreadsheet(SpreadsheetApp.getActive())\n    .onOpen()\n    .create();"
        ),
        "on_edit" => format!(
            "ScriptApp.newTrigger('{func}')\n    .forSpreadsheet(SpreadsheetApp.getActive())\n    .onEdit()\n    .create();"
        ),
        "on_form_submit" => format!(
            "ScriptApp.newTrigger('{func}')\n    .forForm(FormApp.getActiveForm())\n    .onFormSubmit()\n    .create();"
        ),
        other => {
            return Err(format!(
                "Unknown trigger type: {other}. Valid types: time_minutes, time_hours, time_daily, time_weekly, on_open, on_edit, on_form_submit, on_change"
            ));
        }
    };
    Ok(format!(
        "// Trigger setup code for {func}\n// Run createTrigger() once from the Apps Script editor to install it.\n\nfunction createTrigger() {{\n  {}\n}}",
        builder.replace('\n', "\n  ")
    ))
}

pub fn manage_trigger(ctx: &mut Ctx, a: &Value) -> R {
    let sid = s(a, "script_id").to_owned();
    let mut p = need_script(ctx, "manage_script_trigger", &sid)?;
    let mut ts = p["triggers"].as_array().cloned().unwrap_or_default();
    let out = match s(a, "action") {
        "list" => {
            if ts.is_empty() {
                return Ok(format!("No triggers found for script: {sid}"));
            }
            let mut out = vec![format!("Triggers for script: {sid}")];
            out.extend(
                ts.iter()
                    .map(|t| format!("- {} -> {}", s(t, "triggerId"), s(t, "handlerFunction"))),
            );
            return Ok(out.join("\n"));
        }
        "create" => {
            let tid = ctx.hex_id("trigger", 10);
            let h = arg(a, "handler_function")
                .ok_or_else(|| "handler_function is required for create.".to_owned())?;
            ts.push(json!({"triggerId": tid, "handlerFunction": h}));
            format!("Created trigger {tid} for {h} in script: {sid}")
        }
        "delete" => {
            let tid = arg(a, "trigger_id")
                .ok_or_else(|| "trigger_id is required for delete.".to_owned())?
                .to_owned();
            ts.retain(|t| s(t, "triggerId") != tid);
            format!("Deleted trigger {tid} from script: {sid}")
        }
        other => {
            return Err(format!(
                "Invalid action '{other}'. Must be 'list', 'create', or 'delete'."
            ));
        }
    };
    p["triggers"] = json!(ts);
    ctx.put("scripts", p);
    Ok(out)
}

pub fn start_auth(_ctx: &mut Ctx, a: &Value) -> R {
    Ok(format!(
        "**Authentication not needed.** {} is already authenticated for {} — all scopes for the configured tools are granted. Retry your original request.",
        arg(a, "user_google_email").unwrap_or(USER),
        arg(a, "service_name").unwrap_or("Google Workspace")
    ))
}
