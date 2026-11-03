//! `/rest/agile/1.0` (boards, sprints, backlog, epics), `/rest/servicedeskapi` and
//! `/rest/dev-status/1.0`.

use serde_json::{Value, json};

use crate::issues::{
    SPRINT, issue_doc, project_brief, project_doc, render_issue, run_jql, status_obj,
};
use crate::model::{
    Ctx, Out, Req, created, error, field_errors, id_num, iso_z, no_content, not_found_page, ok,
    parse_time, s,
};

fn agile_error(status: u16, msg: &str) -> Out {
    error(status, msg)
}

fn board_obj(ctx: &Ctx, b: &Value) -> Value {
    let mut o = json!({"id": id_num(&b["id"]), "self": format!("{}/rest/agile/1.0/board/{}", ctx.base, s(b, "id")), "name": s(b, "name"), "type": s(b, "type")});
    if let Some(p) = project_doc(ctx, s(b, "project")) {
        o["location"] = json!({"projectId": id_num(&p["id"]), "displayName": format!("{} ({})", s(&p, "name"), s(&p, "key")), "projectName": s(&p, "name"), "projectKey": s(&p, "key"), "projectTypeKey": "software", "avatarURI": format!("/secure/projectavatar?size=small&pid={}", s(&p, "id")), "name": format!("{} ({})", s(&p, "name"), s(&p, "key"))});
    }
    o
}

pub fn sprint_obj(ctx: &Ctx, sp: &Value) -> Value {
    let mut o = json!({"id": id_num(&sp["id"]), "self": format!("{}/rest/agile/1.0/sprint/{}", ctx.base, s(sp, "id")), "state": s(sp, "state"), "name": s(sp, "name"), "originBoardId": id_num(&sp["boardId"])});
    for k in ["startDate", "endDate", "completeDate", "activatedDate"] {
        if !s(sp, k).is_empty() {
            o[k] = json!(s(sp, k));
        }
    }
    if !s(sp, "startDate").is_empty()
        && s(sp, "activatedDate").is_empty()
        && s(sp, "state") != "future"
    {
        o["activatedDate"] = json!(s(sp, "startDate"));
    }
    o["goal"] = json!(s(sp, "goal"));
    o["synced"] = json!(false);
    o["autoStartStop"] = json!(false);
    o
}

fn page(values: Vec<Value>, req: &Req, default: usize) -> Value {
    let start = req.q_i64("startAt").unwrap_or(0).max(0) as usize;
    let max = req.q_i64("maxResults").unwrap_or(default as i64).max(1) as usize;
    let total = values.len();
    let vals: Vec<Value> = values.into_iter().skip(start).take(max).collect();
    json!({"maxResults": max, "startAt": start, "total": total, "isLast": start + vals.len() >= total, "values": vals})
}

fn board_issues(ctx: &Ctx, req: &Req, base_jql: &str) -> Out {
    let extra = req
        .q("jql")
        .map(str::to_owned)
        .or_else(|| req.body["jql"].as_str().map(str::to_owned));
    let (filter, order) = match &extra {
        Some(e) if !e.trim().is_empty() => {
            let lower = e.to_lowercase();
            match lower.find("order by") {
                Some(i) => (e[..i].trim().to_owned(), format!(" {}", &e[i..])),
                None => (e.clone(), String::new()),
            }
        }
        _ => (String::new(), String::new()),
    };
    let q = if filter.is_empty() {
        format!("{base_jql}{order}")
    } else {
        format!("({base_jql}) AND ({filter}){order}")
    };
    let found = match run_jql(ctx, &q) {
        Ok(f) => f,
        Err(e) => return error(400, &e),
    };
    let start = req.q_i64("startAt").unwrap_or(0).max(0) as usize;
    let max = req.q_i64("maxResults").unwrap_or(50).max(1) as usize;
    let fields = req.q("fields").unwrap_or("*navigable").to_owned();
    let issues: Vec<Value> = found
        .iter()
        .skip(start)
        .take(max)
        .map(|i| render_issue(ctx, i, Some(&fields), req.q("expand")))
        .collect();
    ok(
        json!({"expand": "schema,names", "startAt": start, "maxResults": max, "total": found.len(), "issues": issues}),
    )
}

fn set_sprint(ctx: &mut Ctx, key: &str, sprint: Option<i64>) {
    if let Some(mut doc) = issue_doc(ctx, key) {
        let mut list: Vec<Value> = doc["fields"][SPRINT]
            .as_array()
            .cloned()
            .unwrap_or_default();
        // An issue can only be in one open sprint; closed sprints stay in its history.
        list.retain(|x| {
            ctx.get("sprints", &id_num(x).unwrap_or(0).to_string())
                .is_some_and(|sp| s(&sp, "state") == "closed")
        });
        if let Some(n) = sprint {
            list.push(json!(n));
        }
        doc["fields"][SPRINT] = json!(list);
        doc["fields"]["updated"] = json!(ctx.stamp());
        let k = s(&doc, "key").to_owned();
        ctx.put("issues", &k, doc);
    }
}

fn issue_keys(ctx: &Ctx, body: &Value) -> Result<Vec<String>, Out> {
    let list = body["issues"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return Err(field_errors(json!({"issues": "Issues must be specified."})));
    }
    let mut out = Vec::new();
    for k in list {
        let k = k
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| k.to_string());
        match issue_doc(ctx, &k) {
            Some(d) => out.push(s(&d, "key").to_owned()),
            None => {
                return Err(error(
                    400,
                    &format!("Issue does not exist or you do not have permission to see it.: {k}"),
                ));
            }
        }
    }
    Ok(out)
}

fn validate_sprint_dates(body: &Value) -> Result<(), Out> {
    for k in ["startDate", "endDate"] {
        if let Some(d) = body[k].as_str()
            && parse_time(d).is_none()
        {
            return Err(field_errors(
                json!({k: "Invalid date format. Please enter the date in the format \"yyyy-MM-ddTHH:mm:ss.SSSZ\".".to_string()}),
            ));
        }
    }
    Ok(())
}

fn norm_date(v: &str) -> String {
    parse_time(v).map(iso_z).unwrap_or_else(|| v.to_owned())
}

pub fn route(ctx: &mut Ctx, req: &Req, m: &str, rest: &[&str]) -> Out {
    match (m, rest) {
        ("GET", ["board"]) => {
            let name = req.q("name").map(str::to_lowercase);
            let ty = req.q("type");
            let proj = req
                .q("projectKeyOrId")
                .and_then(|p| project_doc(ctx, p))
                .map(|p| s(&p, "key").to_owned());
            if req.q("projectKeyOrId").is_some() && proj.is_none() {
                return agile_error(
                    404,
                    &format!(
                        "No project could be found with key '{}'.",
                        req.q("projectKeyOrId").unwrap_or("")
                    ),
                );
            }
            let boards: Vec<Value> = ctx
                .all("boards")
                .into_iter()
                .filter(|b| {
                    name.as_ref()
                        .is_none_or(|n| s(b, "name").to_lowercase().contains(n))
                })
                .filter(|b| ty.is_none_or(|t| s(b, "type") == t))
                .filter(|b| proj.as_ref().is_none_or(|p| s(b, "project") == p))
                .map(|b| board_obj(ctx, &b))
                .collect();
            ok(page(boards, req, 50))
        }
        ("GET", ["board", id]) => match ctx.get("boards", id) {
            Some(b) => ok(board_obj(ctx, &b)),
            None => agile_error(
                404,
                &format!("Board does not exist or you do not have permission to see it.: {id}"),
            ),
        },
        ("GET", ["board", id, what]) => {
            let Some(b) = ctx.get("boards", id) else {
                return agile_error(
                    404,
                    &format!("Board does not exist or you do not have permission to see it.: {id}"),
                );
            };
            let base = format!("project = {}", s(&b, "project"));
            match *what {
                "issue" => board_issues(ctx, req, &base),
                "backlog" => {
                    let open: Vec<String> = ctx
                        .all("sprints")
                        .into_iter()
                        .filter(|sp| s(sp, "state") != "closed" && s(sp, "boardId") == s(&b, "id"))
                        .map(|sp| s(&sp, "id").to_owned())
                        .collect();
                    let q = if open.is_empty() {
                        format!("{base} AND statusCategory != Done")
                    } else {
                        format!(
                            "{base} AND statusCategory != Done AND (sprint is EMPTY OR sprint not in ({}))",
                            open.join(",")
                        )
                    };
                    board_issues(ctx, req, &q)
                }
                "sprint" => {
                    let states: Vec<String> = req.q_list("state");
                    let sprints: Vec<Value> = ctx
                        .all("sprints")
                        .into_iter()
                        .filter(|sp| s(sp, "boardId") == s(&b, "id"))
                        .filter(|sp| {
                            states.is_empty() || states.iter().any(|st| st == s(sp, "state"))
                        })
                        .map(|sp| sprint_obj(ctx, &sp))
                        .collect();
                    if s(&b, "type") == "kanban" {
                        return agile_error(400, "The board does not support sprints");
                    }
                    ok(page(sprints, req, 50))
                }
                "epic" => {
                    let q = format!("{base} AND issuetype = Epic");
                    let epics: Vec<Value> = run_jql(ctx, &q).unwrap_or_default().iter().map(|e| json!({"id": id_num(&e["id"]), "key": s(e, "key"), "self": format!("{}/rest/agile/1.0/epic/{}", ctx.base, s(e, "id")), "name": e["fields"][crate::issues::EPIC_NAME].as_str().unwrap_or(s(&e["fields"], "summary")), "summary": s(&e["fields"], "summary"), "color": {"key": "color_1"}, "done": crate::issues::status_done(ctx, s(&e["fields"], "status"))})).collect();
                    ok(page(epics, req, 50))
                }
                "configuration" => ok(
                    json!({"id": id_num(&b["id"]), "name": s(&b, "name"), "type": s(&b, "type"), "self": format!("{}/rest/agile/1.0/board/{id}/configuration", ctx.base), "filter": {"id": "10000", "self": ctx.api("filter/10000")}, "columnConfig": {"columns": [{"name": "To Do", "statuses": []}, {"name": "In Progress", "statuses": []}, {"name": "Done", "statuses": []}], "constraintType": "issueCount"}, "estimation": {"type": "field", "field": {"fieldId": crate::issues::STORY_POINTS, "displayName": "Story Points"}}, "ranking": {"rankCustomFieldId": 10105}}),
                ),
                "project" => ok(page(
                    project_doc(ctx, s(&b, "project"))
                        .map(|p| vec![project_brief(ctx, s(&p, "key"))])
                        .unwrap_or_default(),
                    req,
                    50,
                )),
                _ => not_found_page(),
            }
        }
        ("POST", ["sprint"]) => {
            let b = &req.body;
            let name = s(b, "name").trim().to_owned();
            if name.is_empty() {
                return field_errors(json!({"name": "The sprint name must be specified."}));
            }
            let board = id_num(&b["originBoardId"])
                .map(|n| n.to_string())
                .unwrap_or_default();
            let Some(bd) = ctx.get("boards", &board) else {
                return field_errors(
                    json!({"originBoardId": "Board does not exist or you do not have permission to see it."}),
                );
            };
            if s(&bd, "type") == "kanban" {
                return agile_error(400, "The board does not support sprints");
            }
            if let Err(e) = validate_sprint_dates(b) {
                return e;
            }
            let id = ctx.next_id("sprints", 1);
            let mut sp = json!({"id": id, "boardId": board, "name": name, "state": "future", "goal": s(b, "goal")});
            for k in ["startDate", "endDate"] {
                if let Some(d) = b[k].as_str() {
                    sp[k] = json!(norm_date(d));
                }
            }
            ctx.put("sprints", &id, sp.clone());
            created(sprint_obj(ctx, &sp))
        }
        ("GET", ["sprint", id]) => match ctx.get("sprints", id) {
            Some(sp) => ok(sprint_obj(ctx, &sp)),
            None => agile_error(
                404,
                &format!("Sprint does not exist or you do not have permission to see it.: {id}"),
            ),
        },
        ("POST" | "PUT", ["sprint", id]) => {
            let Some(mut sp) = ctx.get("sprints", id) else {
                return agile_error(
                    404,
                    &format!(
                        "Sprint does not exist or you do not have permission to see it.: {id}"
                    ),
                );
            };
            let b = &req.body;
            if let Err(e) = validate_sprint_dates(b) {
                return e;
            }
            if let Some(st) = b["state"].as_str() {
                let cur = s(&sp, "state").to_owned();
                let okay = cur == st
                    || (cur == "future" && st == "active")
                    || (cur == "active" && st == "closed");
                if !okay {
                    return agile_error(
                        400,
                        &format!("A sprint can't be moved from '{cur}' to '{st}'."),
                    );
                }
                if st == "active"
                    && (b["startDate"].is_null() && s(&sp, "startDate").is_empty()
                        || b["endDate"].is_null() && s(&sp, "endDate").is_empty())
                {
                    return field_errors(
                        json!({"startDate": "The sprint must have a start date and an end date to be started."}),
                    );
                }
                if st == "closed" {
                    sp["completeDate"] = json!(iso_z(ctx.now));
                }
                if st == "active" && cur == "future" {
                    sp["activatedDate"] = json!(iso_z(ctx.now));
                }
                sp["state"] = json!(st);
            }
            for k in ["name", "goal"] {
                if let Some(v) = b[k].as_str() {
                    sp[k] = json!(v);
                }
            }
            for k in ["startDate", "endDate"] {
                if let Some(v) = b[k].as_str() {
                    sp[k] = json!(norm_date(v));
                }
            }
            ctx.put("sprints", id, sp.clone());
            ok(sprint_obj(ctx, &sp))
        }
        ("DELETE", ["sprint", id]) => {
            if ctx.remove("sprints", id).is_none() {
                return agile_error(
                    404,
                    &format!(
                        "Sprint does not exist or you do not have permission to see it.: {id}"
                    ),
                );
            }
            no_content()
        }
        ("GET", ["sprint", id, "issue"]) => {
            if ctx.get("sprints", id).is_none() {
                return agile_error(
                    404,
                    &format!(
                        "Sprint does not exist or you do not have permission to see it.: {id}"
                    ),
                );
            }
            board_issues(ctx, req, &format!("sprint = {id}"))
        }
        ("POST", ["sprint", id, "issue"]) => {
            let Some(sp) = ctx.get("sprints", id) else {
                return agile_error(
                    404,
                    &format!(
                        "Sprint does not exist or you do not have permission to see it.: {id}"
                    ),
                );
            };
            if s(&sp, "state") == "closed" {
                return agile_error(400, "Issues can't be moved to a closed sprint.");
            }
            let keys = match issue_keys(ctx, &req.body) {
                Ok(k) => k,
                Err(e) => return e,
            };
            for k in keys {
                set_sprint(ctx, &k, id.parse().ok());
            }
            no_content()
        }
        ("POST", ["backlog", "issue"]) => {
            let keys = match issue_keys(ctx, &req.body) {
                Ok(k) => k,
                Err(e) => return e,
            };
            for k in keys {
                set_sprint(ctx, &k, None);
            }
            no_content()
        }
        ("GET", ["issue", key]) => match issue_doc(ctx, key) {
            Some(i) => ok(render_issue(ctx, &i, req.q("fields"), req.q("expand"))),
            None => error(
                404,
                "Issue does not exist or you do not have permission to see it.",
            ),
        },
        ("GET", ["epic", key]) => {
            match issue_doc(ctx, key).filter(|e| s(&e["fields"], "issuetype") == "Epic") {
                Some(e) => ok(
                    json!({"id": id_num(&e["id"]), "key": s(&e, "key"), "name": e["fields"][crate::issues::EPIC_NAME].as_str().unwrap_or(s(&e["fields"], "summary")), "summary": s(&e["fields"], "summary"), "done": crate::issues::status_done(ctx, s(&e["fields"], "status"))}),
                ),
                None => error(
                    404,
                    "Epic does not exist or you do not have permission to see it.",
                ),
            }
        }
        ("GET", ["epic", key, "issue"]) => {
            board_issues(ctx, req, &format!("\"Epic Link\" = {key}"))
        }
        _ => not_found_page(),
    }
}

// ───────────────────────────── Service Management ─────────────────────────────

fn sd_page(values: Vec<Value>, req: &Req) -> Value {
    let start = req.q_i64("start").unwrap_or(0).max(0) as usize;
    let limit = req.q_i64("limit").unwrap_or(50).max(1) as usize;
    let total = values.len();
    let vals: Vec<Value> = values.into_iter().skip(start).take(limit).collect();
    json!({"size": vals.len(), "start": start, "limit": limit, "isLastPage": start + vals.len() >= total, "values": vals})
}

fn sd_date(t: i64) -> Value {
    let iso = crate::model::stamp(t);
    json!({"iso8601": iso.replace(".000", ""), "jira": iso, "friendly": iso[..10].to_owned(), "epochMillis": t * 1000})
}

fn sd_obj(ctx: &Ctx, d: &Value) -> Value {
    json!({"id": s(d, "id"), "projectId": s(d, "projectId"), "projectName": project_doc(ctx, s(d, "projectKey")).map(|p| s(&p, "name").to_owned()).unwrap_or_default(), "projectKey": s(d, "projectKey"), "_links": {"self": format!("{}/rest/servicedeskapi/servicedesk/{}", ctx.base, s(d, "id"))}})
}

pub fn servicedesk(ctx: &mut Ctx, req: &Req, m: &str, rest: &[&str]) -> Out {
    let missing = |id: &str| error(404, &format!("Service Desk with id '{id}' does not exist."));
    match (m, rest) {
        ("GET", ["servicedesk"]) => {
            let all: Vec<Value> = ctx
                .all("servicedesks")
                .iter()
                .map(|d| sd_obj(ctx, d))
                .collect();
            ok(sd_page(all, req))
        }
        ("GET", ["servicedesk", id]) => match ctx.get("servicedesks", id) {
            Some(d) => ok(sd_obj(ctx, &d)),
            None => missing(id),
        },
        ("GET", ["servicedesk", id, "queue"]) => {
            if ctx.get("servicedesks", id).is_none() {
                return missing(id);
            }
            let with_count = req.q("includeCount") == Some("true");
            let qs: Vec<Value> = ctx
                .all("queues")
                .into_iter()
                .filter(|q| s(q, "serviceDeskId") == *id)
                .map(|q| {
                    let mut o = json!({"id": s(&q, "id"), "name": s(&q, "name"), "jql": s(&q, "jql"), "fields": q.get("fields").cloned().unwrap_or(json!(["issuetype", "issuekey", "summary", "status", "created"])), "_links": {"self": format!("{}/rest/servicedeskapi/servicedesk/{id}/queue/{}", ctx.base, s(&q, "id"))}});
                    if with_count {
                        o["issueCount"] = json!(run_jql(ctx, s(&q, "jql")).map(|v| v.len()).unwrap_or(0));
                    }
                    o
                })
                .collect();
            ok(sd_page(qs, req))
        }
        ("GET", ["servicedesk", id, "queue", qid]) => match ctx
            .get("queues", qid)
            .filter(|q| s(q, "serviceDeskId") == *id)
        {
            Some(q) => ok(
                json!({"id": s(&q, "id"), "name": s(&q, "name"), "jql": s(&q, "jql"), "fields": q.get("fields").cloned().unwrap_or(json!([])), "issueCount": run_jql(ctx, s(&q, "jql")).map(|v| v.len()).unwrap_or(0)}),
            ),
            None => error(404, &format!("Queue with id '{qid}' does not exist.")),
        },
        ("GET", ["servicedesk", id, "queue", qid, "issue"]) => {
            let Some(q) = ctx
                .get("queues", qid)
                .filter(|q| s(q, "serviceDeskId") == *id)
            else {
                return error(404, &format!("Queue with id '{qid}' does not exist."));
            };
            let issues: Vec<Value> = run_jql(ctx, s(&q, "jql"))
                .unwrap_or_default()
                .iter()
                .map(|i| render_issue(ctx, i, Some("*navigable"), None))
                .map(|mut i| {
                    i.as_object_mut().map(|o| o.remove("expand"));
                    i
                })
                .collect();
            ok(sd_page(issues, req))
        }
        ("GET", ["servicedesk", id, "requesttype"]) => {
            if ctx.get("servicedesks", id).is_none() {
                return missing(id);
            }
            let rts: Vec<Value> = ctx.all("requesttypes").into_iter().filter(|r| s(r, "serviceDeskId") == *id).map(|r| json!({"id": s(&r, "id"), "name": s(&r, "name"), "description": s(&r, "description"), "helpText": s(&r, "helpText"), "serviceDeskId": id, "groupIds": r.get("groupIds").cloned().unwrap_or(json!([])), "_links": {"self": format!("{}/rest/servicedeskapi/servicedesk/{id}/requesttype/{}", ctx.base, s(&r, "id"))}})).collect();
            ok(sd_page(rts, req))
        }
        ("GET", ["servicedesk", id, "requesttype", rid, "field"]) => match ctx
            .get("requesttypes", rid)
            .filter(|r| s(r, "serviceDeskId") == *id)
        {
            Some(r) => ok(
                json!({"requestTypeFields": r.get("fields").cloned().unwrap_or(json!([{"fieldId": "summary", "name": "Summary", "description": "", "required": true, "validValues": [], "jiraSchema": {"type": "string", "system": "summary"}}, {"fieldId": "description", "name": "Description", "description": "", "required": false, "validValues": [], "jiraSchema": {"type": "string", "system": "description"}}])), "canRaiseOnBehalfOf": true, "canAddRequestParticipants": true}),
            ),
            None => error(
                404,
                &format!("Request type with id '{rid}' does not exist."),
            ),
        },
        ("POST", ["request"]) => create_request(ctx, &req.body),
        ("GET", ["request", key]) => match issue_doc(ctx, key) {
            Some(i) => ok(request_obj(ctx, &i)),
            None => error(
                404,
                "Request does not exist or you do not have permission to see it.",
            ),
        },
        ("GET", ["request", key, "comment"]) => {
            let Some(i) = issue_doc(ctx, key) else {
                return error(
                    404,
                    "Request does not exist or you do not have permission to see it.",
                );
            };
            let cs: Vec<Value> = ctx
                .all("comments")
                .into_iter()
                .filter(|c| s(c, "issue") == s(&i, "key"))
                .map(|c| sd_comment(ctx, &c))
                .collect();
            ok(sd_page(cs, req))
        }
        ("GET", ["request", key, "comment", cid]) => match ctx
            .get("comments", cid)
            .filter(|c| issue_doc(ctx, key).is_some_and(|i| s(&i, "key") == s(c, "issue")))
        {
            Some(c) => ok(sd_comment(ctx, &c)),
            None => error(404, &format!("Comment with id '{cid}' does not exist.")),
        },
        ("POST", ["request", key, "comment"]) => {
            let Some(i) = issue_doc(ctx, key) else {
                return error(
                    404,
                    "Request does not exist or you do not have permission to see it.",
                );
            };
            let body = s(&req.body, "body").to_owned();
            if body.trim().is_empty() {
                return error(400, "The comment body can not be empty.");
            }
            let id = ctx.next_id("comments", 10000);
            let public = req.body["public"].as_bool().unwrap_or(true);
            let mut c = json!({"id": id, "issue": s(&i, "key"), "author": ctx.me_name(), "body": body, "created": ctx.stamp(), "updated": ctx.stamp(), "public": public});
            if !public {
                c["visibility"] = json!({"type": "role", "value": "Service Desk Team"});
            }
            ctx.put("comments", &id, c.clone());
            created(sd_comment(ctx, &c))
        }
        ("GET", ["request", key, "sla"]) => match issue_doc(ctx, key) {
            Some(i) => ok(sd_page(
                i.get("sla")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                req,
            )),
            None => error(
                404,
                "Request does not exist or you do not have permission to see it.",
            ),
        },
        ("POST", ["request", key, "attachment"]) => match issue_doc(ctx, key) {
            Some(_) => ok(
                json!({"comment": null, "attachments": {"size": 0, "start": 0, "limit": 50, "isLastPage": true, "values": []}}),
            ),
            None => error(
                404,
                "Request does not exist or you do not have permission to see it.",
            ),
        },
        ("GET", ["info"]) => ok(
            json!({"version": "5.12.15", "platformVersion": "9.12.15", "buildDate": sd_date(1_725_148_800), "buildChangeSet": "", "isLicensedForUse": true}),
        ),
        _ => not_found_page(),
    }
}

fn sd_comment(ctx: &Ctx, c: &Value) -> Value {
    let t = parse_time(s(c, "created")).unwrap_or(ctx.now);
    json!({"id": s(c, "id"), "body": s(c, "body"), "public": c["public"].as_bool().unwrap_or(c.get("visibility").is_none_or(Value::is_null)), "author": ctx.user(s(c, "author")), "created": sd_date(t), "_links": {"self": format!("{}/rest/servicedeskapi/request/{}/comment/{}", ctx.base, s(c, "issue"), s(c, "id"))}})
}

fn request_obj(ctx: &Ctx, i: &Value) -> Value {
    let f = &i["fields"];
    let t = parse_time(s(f, "created")).unwrap_or(ctx.now);
    let st = status_obj(ctx, s(f, "status"));
    json!({
        "issueId": s(i, "id"), "issueKey": s(i, "key"), "requestTypeId": s(i, "requestTypeId"), "serviceDeskId": s(i, "serviceDeskId"),
        "createdDate": sd_date(t), "reporter": ctx.user(s(f, "reporter")),
        "requestFieldValues": [{"fieldId": "summary", "label": "Summary", "value": s(f, "summary")}, {"fieldId": "description", "label": "Description", "value": f["description"]}],
        "currentStatus": {"status": s(&st, "name"), "statusCategory": st["statusCategory"]["key"].as_str().unwrap_or("new").to_uppercase(), "statusDate": sd_date(t)},
        "_links": {"web": format!("{}/servicedesk/customer/portal/{}/{}", ctx.base, s(i, "serviceDeskId"), s(i, "key")), "self": format!("{}/rest/servicedeskapi/request/{}", ctx.base, s(i, "id"))},
    })
}

fn create_request(ctx: &mut Ctx, b: &Value) -> Out {
    let sd = b["serviceDeskId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| b["serviceDeskId"].to_string());
    let Some(desk) = ctx.get("servicedesks", &sd) else {
        return error(404, &format!("Service Desk with id '{sd}' does not exist."));
    };
    let rt = b["requestTypeId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| b["requestTypeId"].to_string());
    let Some(rtype) = ctx
        .get("requesttypes", &rt)
        .filter(|r| s(r, "serviceDeskId") == sd)
    else {
        return error(
            400,
            &format!("The request type '{rt}' does not exist in this service desk."),
        );
    };
    let values = b["requestFieldValues"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    if values
        .get("summary")
        .and_then(Value::as_str)
        .is_none_or(|x| x.trim().is_empty())
    {
        return field_errors(json!({"summary": "Summary is required."}));
    }
    let mut fields = serde_json::Map::new();
    fields.insert("project".into(), json!({"key": s(&desk, "projectKey")}));
    fields.insert("issuetype".into(), json!({"name": if s(&rtype, "issueType").is_empty() { "Service Request" } else { s(&rtype, "issueType") }}));
    for (k, v) in values {
        fields.insert(k, v);
    }
    let reporter = b["raiseOnBehalfOf"].as_str().map(str::to_owned);
    let (key, _) = match crate::issues::create_issue(ctx, &json!({"fields": fields})) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let mut doc = issue_doc(ctx, &key).expect("created issue");
    doc["serviceDeskId"] = json!(sd);
    doc["requestTypeId"] = json!(rt);
    if let Some(r) = reporter.and_then(|r| ctx.user_doc(&r)) {
        doc["fields"]["reporter"] = json!(s(&r, "name"));
    }
    ctx.put("issues", &key, doc.clone());
    created(request_obj(ctx, &doc))
}

pub fn dev_status(ctx: &mut Ctx, req: &Req, m: &str, rest: &[&str]) -> Out {
    let issue = req.q("issueId").and_then(|id| issue_doc(ctx, id));
    match (m, rest) {
        ("GET", ["issue", "detail"]) => match issue {
            Some(i) => {
                let details = i
                    .get("development")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                ok(json!({"errors": [], "detail": details}))
            }
            None => ok(json!({"errors": [], "detail": []})),
        },
        ("GET", ["issue", "summary"]) => {
            let zero = json!({"overall": {"count": 0, "lastUpdated": null, "dataType": "", "stateCount": 0, "state": null, "details": {}}, "byInstanceType": {}});
            ok(
                json!({"errors": [], "configErrors": [], "summary": {"pullrequest": zero, "build": zero, "review": zero, "deployment-environment": zero, "repository": zero, "branch": zero}}),
            )
        }
        _ => not_found_page(),
    }
}
