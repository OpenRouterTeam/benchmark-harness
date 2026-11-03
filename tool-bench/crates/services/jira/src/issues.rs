//! `/rest/api/2`: issues and everything hanging off them, search, metadata, projects, users.

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::jql;
use crate::model::{
    Ctx, Out, Req, created, error, field_errors, format_duration, id_num, issue_missing,
    no_content, not_found_page, ok, parse_time, s, stamp,
};

pub const EPIC_LINK: &str = "customfield_10100";
pub const EPIC_NAME: &str = "customfield_10102";
pub const SPRINT: &str = "customfield_10104";
pub const STORY_POINTS: &str = "customfield_10106";
pub const RANK: &str = "customfield_10105";

pub fn route(ctx: &mut Ctx, req: &Req, m: &str, rest: &[&str]) -> Out {
    match (m, rest) {
        ("GET", ["myself"]) => {
            let me = ctx.me_name();
            let mut u = ctx.user(&me);
            u["groups"] = json!({"size": 2, "items": []});
            u["applicationRoles"] = json!({"size": 1, "items": []});
            u["expand"] = json!("groups,applicationRoles");
            ok(u)
        }
        ("GET", ["serverInfo"]) => ok(
            json!({"baseUrl": ctx.base, "version": "9.12.15", "versionNumbers": [9, 12, 15], "deploymentType": "Server", "buildNumber": 9120015, "buildDate": "2024-09-01T00:00:00.000+0000", "serverTime": ctx.stamp(), "scmInfo": "", "serverTitle": "Octo Corp Jira"}),
        ),
        ("GET", ["user"]) => {
            let who = req
                .q("username")
                .or(req.q("key"))
                .or(req.q("accountId"))
                .unwrap_or("");
            match ctx.user_doc(who) {
                Some(u) => {
                    let mut out = ctx.user(s(&u, "name"));
                    out["groups"] = json!({"size": 1, "items": [{"name": "jira-software-users"}]});
                    out["applicationRoles"] = json!({"size": 1, "items": []});
                    out["expand"] = json!("groups,applicationRoles");
                    ok(out)
                }
                None => error(404, &format!("The user named '{who}' does not exist")),
            }
        }
        (
            "GET",
            [
                "user",
                "search" | "assignable" | "viewissue" | "permission",
                ..,
            ],
        ) => user_search(ctx, req, rest),
        ("GET", ["user", "picker"]) => {
            let users = find_users(ctx, req.q("query").unwrap_or(""));
            let list: Vec<Value> = users.iter().map(|u| json!({"name": s(u, "name"), "key": s(u, "name"), "html": format!("{} - {} ({})", s(u, "displayName"), s(u, "emailAddress"), s(u, "name")), "displayName": s(u, "displayName"), "avatarUrl": format!("{}/secure/useravatar?size=small&ownerId={}", ctx.base, s(u, "name"))})).collect();
            ok(
                json!({"users": list, "total": list.len(), "header": format!("Showing {} of {} matching users", list.len(), list.len())}),
            )
        }
        ("GET", ["field"]) => ok(json!(all_fields(ctx))),
        ("GET", ["field", _, ..]) => not_found_page(),
        ("GET", ["status"]) => ok(json!(
            ctx.all("statuses")
                .iter()
                .map(|x| status_obj(ctx, s(x, "name")))
                .collect::<Vec<_>>()
        )),
        ("GET", ["status", id]) => match find_named(ctx, "statuses", id) {
            Some(x) => ok(status_obj(ctx, s(&x, "name"))),
            None => error(404, &format!("The status with id '{id}' does not exist")),
        },
        ("GET", ["priority"]) => ok(json!(
            ctx.all("priorities")
                .iter()
                .map(|x| priority_obj(ctx, s(x, "name")))
                .collect::<Vec<_>>()
        )),
        ("GET", ["resolution"]) => ok(json!(
            ctx.all("resolutions")
                .iter()
                .map(|x| resolution_obj(ctx, s(x, "name")))
                .collect::<Vec<_>>()
        )),
        ("GET", ["issuetype"]) => ok(json!(
            ctx.all("issuetypes")
                .iter()
                .map(|x| issuetype_obj(ctx, s(x, "name")))
                .collect::<Vec<_>>()
        )),
        ("GET", ["issueLinkType"]) => ok(
            json!({"issueLinkTypes": ctx.all("issuelinktypes").iter().map(|t| link_type_obj(ctx, t)).collect::<Vec<_>>()}),
        ),
        ("GET" | "POST", ["search"]) => search(ctx, req),
        ("POST", ["issueLink"]) => create_link(ctx, req),
        ("GET", ["issueLink", id]) => match ctx.get("issuelinks", id) {
            Some(l) => ok(link_full(ctx, &l)),
            None => error(404, &format!("No issue link with id '{id}' exists.")),
        },
        ("DELETE", ["issueLink", id]) => {
            if ctx.remove("issuelinks", id).is_none() {
                return error(404, &format!("No issue link with id '{id}' exists."));
            }
            no_content()
        }
        ("POST", ["issue"]) => create_issue(ctx, &req.body).map_or_else(|e| e, |(_, v)| created(v)),
        ("POST", ["issue", "bulk"]) => bulk_create(ctx, req),
        ("GET", ["issue", "createmeta"]) => createmeta_old(ctx, req),
        ("GET", ["issue", "createmeta", project, "issuetypes"]) => createmeta_types(ctx, project),
        ("GET", ["issue", "createmeta", project, "issuetypes", type_id]) => {
            createmeta_fields(ctx, project, type_id)
        }
        (_, ["issue", key, tail @ ..]) => issue_route(ctx, req, m, key, tail),
        ("GET", ["attachment", "meta"]) => ok(json!({"enabled": true, "uploadLimit": 10_485_760})),
        ("GET", ["attachment", id]) => match ctx.get("attachments", id) {
            Some(a) => ok(attachment_obj(ctx, &a)),
            None => error(
                404,
                &format!("The attachment with id '{id}' does not exist"),
            ),
        },
        ("DELETE", ["attachment", id]) => {
            if ctx.remove("attachments", id).is_none() {
                return error(
                    404,
                    &format!("The attachment with id '{id}' does not exist"),
                );
            }
            no_content()
        }
        ("GET", ["project"]) => ok(json!(
            ctx.all("projects")
                .iter()
                .map(|p| project_obj(ctx, p, false))
                .collect::<Vec<_>>()
        )),
        ("GET", ["projects", "picker"]) => {
            let q = req.q("query").unwrap_or("").to_lowercase();
            let list: Vec<Value> = ctx.all("projects").into_iter().filter(|p| s(p, "key").to_lowercase().contains(&q) || s(p, "name").to_lowercase().contains(&q)).map(|p| json!({"key": s(&p, "key"), "name": s(&p, "name"), "id": s(&p, "id"), "html": format!("{} ({})", s(&p, "name"), s(&p, "key")), "avatar": format!("{}/secure/projectavatar?pid={}", ctx.base, s(&p, "id"))})).collect();
            ok(
                json!({"header": format!("Showing {} of {} matching projects", list.len(), list.len()), "total": list.len(), "projects": list}),
            )
        }
        (_, ["project", key, tail @ ..]) => project_route(ctx, req, m, key, tail),
        ("POST", ["version"]) => create_version(ctx, &req.body),
        ("GET", ["version", id]) => match ctx.get("versions", id) {
            Some(v) => ok(version_obj(ctx, &v)),
            None => error(404, &format!("Could not find version for id '{id}'")),
        },
        ("PUT", ["version", id]) => update_version(ctx, id, &req.body),
        ("GET", ["component", id]) => match component_by_id(ctx, id) {
            Some((_, c)) => ok(c),
            None => error(404, &format!("The component with id {id} does not exist.")),
        },
        _ => not_found_page(),
    }
}

// ───────────────────────────── lookups and objects ─────────────────────────────

pub fn find_named(ctx: &Ctx, coll: &str, v: &str) -> Option<Value> {
    ctx.all(coll)
        .into_iter()
        .find(|x| s(x, "name").eq_ignore_ascii_case(v) || s(x, "id") == v)
}

fn status_category(cat: &str) -> Value {
    let (id, name, color) = match cat {
        "done" => (3, "Done", "green"),
        "indeterminate" => (4, "In Progress", "yellow"),
        _ => (2, "To Do", "blue-gray"),
    };
    json!({"id": id, "key": if cat.is_empty() { "new" } else { cat }, "colorName": color, "name": name})
}

pub fn status_obj(ctx: &Ctx, name: &str) -> Value {
    let st = find_named(ctx, "statuses", name)
        .unwrap_or(json!({"id": "1", "name": name, "statusCategory": "new"}));
    let mut cat = status_category(s(&st, "statusCategory"));
    cat["self"] = json!(ctx.api(&format!("statuscategory/{}", cat["id"])));
    json!({"self": ctx.api(&format!("status/{}", s(&st, "id"))), "description": s(&st, "description"), "iconUrl": format!("{}/images/icons/statuses/generic.png", ctx.base), "name": s(&st, "name"), "id": s(&st, "id"), "statusCategory": cat})
}

pub fn status_done(ctx: &Ctx, name: &str) -> bool {
    find_named(ctx, "statuses", name).is_some_and(|x| s(&x, "statusCategory") == "done")
}

pub fn priority_obj(ctx: &Ctx, name: &str) -> Value {
    match find_named(ctx, "priorities", name) {
        Some(p) => {
            json!({"self": ctx.api(&format!("priority/{}", s(&p, "id"))), "iconUrl": format!("{}/images/icons/priorities/{}.svg", ctx.base, s(&p, "name").to_lowercase()), "name": s(&p, "name"), "id": s(&p, "id")})
        }
        None => Value::Null,
    }
}

fn resolution_obj(ctx: &Ctx, name: &str) -> Value {
    match find_named(ctx, "resolutions", name) {
        Some(r) => {
            json!({"self": ctx.api(&format!("resolution/{}", s(&r, "id"))), "id": s(&r, "id"), "description": s(&r, "description"), "name": s(&r, "name")})
        }
        None => Value::Null,
    }
}

pub fn issuetype_obj(ctx: &Ctx, name: &str) -> Value {
    match find_named(ctx, "issuetypes", name) {
        Some(t) => {
            json!({"self": ctx.api(&format!("issuetype/{}", s(&t, "id"))), "id": s(&t, "id"), "description": s(&t, "description"), "iconUrl": format!("{}/secure/viewavatar?size=xsmall&avatarId={}&avatarType=issuetype", ctx.base, 10300 + id_num(&t["id"]).unwrap_or(0)), "name": s(&t, "name"), "subtask": t["subtask"].as_bool().unwrap_or(false), "avatarId": 10300 + id_num(&t["id"]).unwrap_or(0)})
        }
        None => Value::Null,
    }
}

fn link_type_obj(ctx: &Ctx, t: &Value) -> Value {
    json!({"id": s(t, "id"), "name": s(t, "name"), "inward": s(t, "inward"), "outward": s(t, "outward"), "self": ctx.api(&format!("issueLinkType/{}", s(t, "id")))})
}

pub fn project_doc(ctx: &Ctx, key: &str) -> Option<Value> {
    ctx.get("projects", &key.to_uppercase()).or_else(|| {
        ctx.all("projects")
            .into_iter()
            .find(|p| s(p, "id") == key || s(p, "name").eq_ignore_ascii_case(key))
    })
}

pub fn project_brief(ctx: &Ctx, key: &str) -> Value {
    let Some(p) = project_doc(ctx, key) else {
        return Value::Null;
    };
    json!({"self": ctx.api(&format!("project/{}", s(&p, "id"))), "id": s(&p, "id"), "key": s(&p, "key"), "name": s(&p, "name"), "projectTypeKey": if s(&p, "projectTypeKey").is_empty() { "software" } else { s(&p, "projectTypeKey") }, "avatarUrls": project_avatars(&ctx.base, s(&p, "id")), "projectCategory": p.get("category").cloned().unwrap_or(Value::Null)})
}

fn project_avatars(base: &str, id: &str) -> Value {
    let mut m = Map::new();
    for (k, size) in [
        ("48x48", ""),
        ("24x24", "size=small&"),
        ("16x16", "size=xsmall&"),
        ("32x32", "size=medium&"),
    ] {
        m.insert(
            k.into(),
            json!(format!(
                "{base}/secure/projectavatar?{size}pid={id}&avatarId=10324"
            )),
        );
    }
    Value::Object(m)
}

fn project_types(ctx: &Ctx, p: &Value) -> Vec<Value> {
    let allowed: Vec<String> = p["issueTypes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    ctx.all("issuetypes")
        .into_iter()
        .filter(|t| {
            allowed.is_empty()
                || allowed
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(s(t, "name")) || a == s(t, "id"))
        })
        .collect()
}

fn project_obj(ctx: &Ctx, p: &Value, full: bool) -> Value {
    let mut out = project_brief(ctx, s(p, "key"));
    out["expand"] = json!("description,lead,url,projectKeys");
    if full {
        out["description"] = json!(s(p, "description"));
        out["lead"] = ctx.user(s(p, "lead"));
        out["components"] = json!(components(ctx, s(p, "key")));
        out["issueTypes"] = json!(
            project_types(ctx, p)
                .iter()
                .map(|t| issuetype_obj(ctx, s(t, "name")))
                .collect::<Vec<_>>()
        );
        out["assigneeType"] = json!("PROJECT_LEAD");
        out["versions"] = json!(
            versions(ctx, s(p, "key"))
                .iter()
                .map(|v| version_obj(ctx, v))
                .collect::<Vec<_>>()
        );
        let roles: Map<String, Value> = role_names(p)
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    r,
                    json!(ctx.api(&format!("project/{}/role/{}", s(p, "id"), 10002 + i))),
                )
            })
            .collect();
        out["roles"] = Value::Object(roles);
        out["archived"] = json!(false);
    }
    out
}

fn role_names(p: &Value) -> Vec<String> {
    p["roles"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_else(|| vec!["Administrators".into(), "Developers".into(), "Users".into()])
}

fn components(ctx: &Ctx, key: &str) -> Vec<Value> {
    let Some(p) = project_doc(ctx, key) else {
        return vec![];
    };
    p["components"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let mut o = json!({"self": ctx.api(&format!("component/{}", s(c, "id"))), "id": s(c, "id"), "name": s(c, "name"), "description": s(c, "description"), "assigneeType": "PROJECT_DEFAULT", "realAssigneeType": "PROJECT_DEFAULT", "isAssigneeTypeValid": false, "project": s(&p, "key"), "projectId": id_num(&p["id"]).unwrap_or(0), "archived": false});
            if !s(c, "lead").is_empty() {
                o["lead"] = ctx.user(s(c, "lead"));
            }
            o
        })
        .collect()
}

fn component_by_id(ctx: &Ctx, id: &str) -> Option<(String, Value)> {
    for p in ctx.all("projects") {
        if let Some(c) = components(ctx, s(&p, "key"))
            .into_iter()
            .find(|c| s(c, "id") == id)
        {
            return Some((s(&p, "key").to_owned(), c));
        }
    }
    None
}

fn versions(ctx: &Ctx, key: &str) -> Vec<Value> {
    let pk = project_doc(ctx, key)
        .map(|p| s(&p, "key").to_owned())
        .unwrap_or_default();
    ctx.all("versions")
        .into_iter()
        .filter(|v| s(v, "project") == pk)
        .collect()
}

fn version_obj(ctx: &Ctx, v: &Value) -> Value {
    let pid = project_doc(ctx, s(v, "project"))
        .map(|p| id_num(&p["id"]).unwrap_or(0))
        .unwrap_or(0);
    let mut o = json!({"self": ctx.api(&format!("version/{}", s(v, "id"))), "id": s(v, "id"), "name": s(v, "name"), "archived": v["archived"].as_bool().unwrap_or(false), "released": v["released"].as_bool().unwrap_or(false), "projectId": pid});
    for k in ["description", "startDate", "releaseDate"] {
        if !s(v, k).is_empty() {
            o[k] = json!(s(v, k));
        }
    }
    if let Some(d) = v.get("releaseDate").and_then(Value::as_str)
        && let Some(t) = parse_time(d)
    {
        o["overdue"] = json!(!v["released"].as_bool().unwrap_or(false) && t < ctx.now);
        o["userReleaseDate"] = json!(user_date(t));
    }
    o
}

fn user_date(t: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, m, d) = crate::model::civil(t.div_euclid(86_400));
    format!("{d:02}/{}/{:02}", MONTHS[(m - 1) as usize], y % 100)
}

pub fn sprint_doc(ctx: &Ctx, id: &Value) -> Option<Value> {
    let key = match id {
        Value::Number(n) => n.to_string(),
        Value::String(x) => x.clone(),
        _ => return None,
    };
    ctx.get("sprints", &key)
}

fn sprint_string(sp: &Value) -> String {
    let opt = |k: &str| {
        if s(sp, k).is_empty() {
            "<null>".to_owned()
        } else {
            s(sp, k).to_owned()
        }
    };
    format!(
        "com.atlassian.greenhopper.service.sprint.Sprint@{:x}[id={},rapidViewId={},state={},name={},startDate={},endDate={},completeDate={},activatedDate={},sequence={},goal={},autoStartStop=false,synced=false]",
        0x1f00_0000 + id_num(&sp["id"]).unwrap_or(0),
        id_num(&sp["id"]).unwrap_or(0),
        id_num(&sp["boardId"]).unwrap_or(0),
        s(sp, "state").to_uppercase(),
        s(sp, "name"),
        opt("startDate"),
        opt("endDate"),
        opt("completeDate"),
        opt("startDate"),
        id_num(&sp["id"]).unwrap_or(0),
        s(sp, "goal")
    )
}

// ───────────────────────────── users ─────────────────────────────

pub fn find_users(ctx: &Ctx, q: &str) -> Vec<Value> {
    let ql = q.trim().to_lowercase();
    ctx.all("users")
        .into_iter()
        .filter(|u| u["active"].as_bool().unwrap_or(true) || ql == ".")
        .filter(|u| {
            if ql == "." || ql.is_empty() {
                return true;
            }
            let hay = [s(u, "name"), s(u, "displayName"), s(u, "emailAddress")];
            hay.iter().any(|h| {
                let h = h.to_lowercase();
                h.starts_with(&ql)
                    || h.split([' ', '.', '@', '-']).any(|w| w.starts_with(&ql))
                    || h == ql
            })
        })
        .collect()
}

fn user_search(ctx: &Ctx, req: &Req, rest: &[&str]) -> Out {
    let q = req.q("username").or(req.q("query")).unwrap_or("");
    if q.is_empty() && rest.first() == Some(&"search") {
        return error(400, "The username query parameter was not provided");
    }
    let mut users = find_users(ctx, q);
    if let Some(pk) = req.q("project").or(req.q("projectKey"))
        && project_doc(ctx, pk).is_none()
    {
        return error(404, &format!("No project could be found with key '{pk}'."));
    }
    if let Some(ik) = req.q("issueKey")
        && ctx.get("issues", &ik.to_uppercase()).is_none()
    {
        return issue_missing();
    }
    let start = req.q_i64("startAt").unwrap_or(0).max(0) as usize;
    let max = req.q_i64("maxResults").unwrap_or(50).max(1) as usize;
    users = users.into_iter().skip(start).take(max).collect();
    let list: Vec<Value> = users.iter().map(|u| ctx.user(s(u, "name"))).collect();
    if rest.first() == Some(&"permission") && req.q("query").is_some() {
        return ok(json!({"users": list}));
    }
    ok(json!(list))
}

// ───────────────────────────── fields ─────────────────────────────

fn sys(
    id: &str,
    name: &str,
    ty: &str,
    items: Option<&str>,
    clauses: &[&str],
    orderable: bool,
) -> Value {
    let mut schema = json!({"type": ty, "system": id});
    if let Some(i) = items {
        schema["items"] = json!(i);
    }
    json!({"id": id, "name": name, "custom": false, "orderable": orderable, "navigable": true, "searchable": true, "clauseNames": clauses, "schema": schema})
}

pub fn all_fields(ctx: &Ctx) -> Vec<Value> {
    let mut out = vec![
        sys(
            "issuekey",
            "Key",
            "string",
            None,
            &["id", "issue", "issuekey", "key"],
            false,
        ),
        sys("summary", "Summary", "string", None, &["summary"], true),
        sys(
            "description",
            "Description",
            "string",
            None,
            &["description"],
            true,
        ),
        sys(
            "issuetype",
            "Issue Type",
            "issuetype",
            None,
            &["issuetype", "type"],
            true,
        ),
        sys("project", "Project", "project", None, &["project"], false),
        sys("status", "Status", "status", None, &["status"], false),
        sys(
            "priority",
            "Priority",
            "priority",
            None,
            &["priority"],
            true,
        ),
        sys(
            "resolution",
            "Resolution",
            "resolution",
            None,
            &["resolution"],
            true,
        ),
        sys("assignee", "Assignee", "user", None, &["assignee"], true),
        sys("reporter", "Reporter", "user", None, &["reporter"], true),
        sys("creator", "Creator", "user", None, &["creator"], false),
        sys(
            "labels",
            "Labels",
            "array",
            Some("string"),
            &["labels"],
            true,
        ),
        sys(
            "components",
            "Component/s",
            "array",
            Some("component"),
            &["component"],
            true,
        ),
        sys(
            "fixVersions",
            "Fix Version/s",
            "array",
            Some("version"),
            &["fixVersion"],
            true,
        ),
        sys(
            "versions",
            "Affects Version/s",
            "array",
            Some("version"),
            &["affectedVersion"],
            true,
        ),
        sys(
            "created",
            "Created",
            "datetime",
            None,
            &["created", "createdDate"],
            false,
        ),
        sys(
            "updated",
            "Updated",
            "datetime",
            None,
            &["updated", "updatedDate"],
            false,
        ),
        sys(
            "duedate",
            "Due Date",
            "date",
            None,
            &["due", "duedate"],
            true,
        ),
        sys(
            "resolutiondate",
            "Resolved",
            "datetime",
            None,
            &["resolutiondate", "resolved"],
            false,
        ),
        sys(
            "environment",
            "Environment",
            "string",
            None,
            &["environment"],
            true,
        ),
        sys(
            "comment",
            "Comment",
            "comments-page",
            None,
            &["comment"],
            true,
        ),
        sys("worklog", "Log Work", "array", Some("worklog"), &[], true),
        sys(
            "attachment",
            "Attachment",
            "array",
            Some("attachment"),
            &["attachments"],
            true,
        ),
        sys(
            "issuelinks",
            "Linked Issues",
            "array",
            Some("issuelinks"),
            &["issueLink"],
            true,
        ),
        sys(
            "subtasks",
            "Sub-Tasks",
            "array",
            Some("issuelinks"),
            &["subtasks"],
            false,
        ),
        sys("parent", "Parent", "issuelink", None, &["parent"], false),
        sys(
            "timetracking",
            "Time Tracking",
            "timetracking",
            None,
            &[],
            true,
        ),
        sys("watches", "Watchers", "watches", None, &["watchers"], false),
        sys("votes", "Votes", "votes", None, &["votes"], false),
        sys(
            "security",
            "Security Level",
            "securitylevel",
            None,
            &["level"],
            true,
        ),
    ];
    let custom = [
        (
            EPIC_LINK,
            "Epic Link",
            "any",
            "com.pyxis.greenhopper.jira:gh-epic-link",
            10100,
        ),
        (
            EPIC_NAME,
            "Epic Name",
            "string",
            "com.pyxis.greenhopper.jira:gh-epic-label",
            10102,
        ),
        (
            "customfield_10103",
            "Epic Status",
            "option",
            "com.pyxis.greenhopper.jira:gh-epic-status",
            10103,
        ),
        (
            SPRINT,
            "Sprint",
            "array",
            "com.pyxis.greenhopper.jira:gh-sprint",
            10104,
        ),
        (
            RANK,
            "Rank",
            "any",
            "com.pyxis.greenhopper.jira:gh-lexo-rank",
            10105,
        ),
        (
            STORY_POINTS,
            "Story Points",
            "number",
            "com.atlassian.jira.plugin.system.customfieldtypes:float",
            10106,
        ),
    ];
    for (id, name, ty, cust, num) in custom {
        out.push(json!({"id": id, "name": name, "custom": true, "orderable": true, "navigable": true, "searchable": true, "clauseNames": [format!("cf[{num}]"), name], "schema": {"type": ty, "custom": cust, "customId": num}}));
    }
    for f in ctx.all("fields") {
        out.push(f);
    }
    out
}

fn field_id_for(ctx: &Ctx, name: &str) -> Option<String> {
    let n = name.to_lowercase();
    all_fields(ctx)
        .into_iter()
        .find(|f| {
            s(f, "id").eq_ignore_ascii_case(&n)
                || s(f, "name").eq_ignore_ascii_case(&n)
                || f["clauseNames"].as_array().is_some_and(|c| {
                    c.iter()
                        .any(|x| x.as_str().is_some_and(|x| x.eq_ignore_ascii_case(&n)))
                })
        })
        .map(|f| s(&f, "id").to_owned())
}

// ───────────────────────────── issue rendering ─────────────────────────────

pub fn issue_doc(ctx: &Ctx, key: &str) -> Option<Value> {
    ctx.get("issues", &key.to_uppercase())
        .or_else(|| ctx.all("issues").into_iter().find(|i| s(i, "id") == key))
}

fn user_ref(ctx: &Ctx, f: &Value, k: &str) -> Value {
    ctx.user_or_null(&f[k])
}

fn comments_of(ctx: &Ctx, key: &str) -> Vec<Value> {
    ctx.all("comments")
        .into_iter()
        .filter(|c| s(c, "issue") == key)
        .collect()
}

fn comment_obj(ctx: &Ctx, c: &Value) -> Value {
    let issue_id = issue_doc(ctx, s(c, "issue"))
        .map(|i| s(&i, "id").to_owned())
        .unwrap_or_default();
    let mut o = json!({"self": ctx.api(&format!("issue/{issue_id}/comment/{}", s(c, "id"))), "id": s(c, "id"), "author": ctx.user(s(c, "author")), "body": s(c, "body"), "updateAuthor": ctx.user(if s(c, "updateAuthor").is_empty() { s(c, "author") } else { s(c, "updateAuthor") }), "created": s(c, "created"), "updated": if s(c, "updated").is_empty() { s(c, "created") } else { s(c, "updated") }});
    if let Some(v) = c.get("visibility").filter(|v| !v.is_null()) {
        o["visibility"] = v.clone();
    }
    o
}

fn worklog_obj(ctx: &Ctx, w: &Value) -> Value {
    let issue_id = issue_doc(ctx, s(w, "issue"))
        .map(|i| s(&i, "id").to_owned())
        .unwrap_or_default();
    let secs = w["timeSpentSeconds"].as_i64().unwrap_or(0);
    json!({"self": ctx.api(&format!("issue/{issue_id}/worklog/{}", s(w, "id"))), "author": ctx.user(s(w, "author")), "updateAuthor": ctx.user(s(w, "author")), "comment": s(w, "comment"), "created": s(w, "created"), "updated": s(w, "created"), "started": s(w, "started"), "timeSpent": format_duration(secs), "timeSpentSeconds": secs, "id": s(w, "id"), "issueId": issue_id})
}

fn brief_issue(ctx: &Ctx, key: &str) -> Value {
    let Some(i) = issue_doc(ctx, key) else {
        return Value::Null;
    };
    let f = &i["fields"];
    json!({"id": s(&i, "id"), "key": s(&i, "key"), "self": ctx.api(&format!("issue/{}", s(&i, "id"))), "fields": {"summary": s(f, "summary"), "status": status_obj(ctx, s(f, "status")), "priority": priority_obj(ctx, s(f, "priority")), "issuetype": issuetype_obj(ctx, s(f, "issuetype"))}})
}

fn link_full(ctx: &Ctx, l: &Value) -> Value {
    let t = ctx.get("issuelinktypes", s(l, "type")).unwrap_or(json!({}));
    json!({"id": s(l, "id"), "self": ctx.api(&format!("issueLink/{}", s(l, "id"))), "type": link_type_obj(ctx, &t), "inwardIssue": brief_issue(ctx, s(l, "inward")), "outwardIssue": brief_issue(ctx, s(l, "outward"))})
}

fn links_of(ctx: &Ctx, key: &str) -> Vec<Value> {
    ctx.all("issuelinks")
        .into_iter()
        .filter(|l| s(l, "inward") == key || s(l, "outward") == key)
        .map(|l| {
            let t = ctx.get("issuelinktypes", s(&l, "type")).unwrap_or(json!({}));
            let mut o = json!({"id": s(&l, "id"), "self": ctx.api(&format!("issueLink/{}", s(&l, "id"))), "type": link_type_obj(ctx, &t)});
            // Stored as created: `outward` is the source ("outward blocks inward").
            if s(&l, "inward") == key {
                o["inwardIssue"] = brief_issue(ctx, s(&l, "outward"));
            } else {
                o["outwardIssue"] = brief_issue(ctx, s(&l, "inward"));
            }
            o
        })
        .collect()
}

pub fn attachment_obj(ctx: &Ctx, a: &Value) -> Value {
    json!({"self": ctx.api(&format!("attachment/{}", s(a, "id"))), "id": s(a, "id"), "filename": s(a, "filename"), "author": ctx.user(s(a, "author")), "created": s(a, "created"), "size": a["size"].as_i64().unwrap_or(s(a, "content").len() as i64), "mimeType": s(a, "mimeType"), "content": format!("{}/secure/attachment/{}/{}", ctx.base, s(a, "id"), s(a, "filename"))})
}

/// Renders every field of an issue as the API would.
fn render_fields(ctx: &Ctx, i: &Value) -> Map<String, Value> {
    let f = &i["fields"];
    let key = s(i, "key");
    let mut out = Map::new();
    let pkey = s(f, "project");
    out.insert("project".into(), project_brief(ctx, pkey));
    out.insert("issuetype".into(), issuetype_obj(ctx, s(f, "issuetype")));
    out.insert("summary".into(), json!(s(f, "summary")));
    out.insert(
        "description".into(),
        f.get("description").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "environment".into(),
        f.get("environment").cloned().unwrap_or(Value::Null),
    );
    out.insert("status".into(), status_obj(ctx, s(f, "status")));
    out.insert("priority".into(), priority_obj(ctx, s(f, "priority")));
    out.insert(
        "resolution".into(),
        match f["resolution"].as_str() {
            Some(r) if !r.is_empty() => resolution_obj(ctx, r),
            _ => Value::Null,
        },
    );
    out.insert(
        "resolutiondate".into(),
        f.get("resolutiondate").cloned().unwrap_or(Value::Null),
    );
    out.insert("assignee".into(), user_ref(ctx, f, "assignee"));
    out.insert("reporter".into(), user_ref(ctx, f, "reporter"));
    out.insert(
        "creator".into(),
        ctx.user_or_null(if f["creator"].is_null() {
            &f["reporter"]
        } else {
            &f["creator"]
        }),
    );
    out.insert(
        "labels".into(),
        f.get("labels").cloned().unwrap_or(json!([])),
    );
    let comps = components(ctx, pkey);
    let names = |k: &str| -> Vec<String> {
        f[k].as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    out.insert("components".into(), json!(names("components").iter().filter_map(|n| comps.iter().find(|c| s(c, "name") == n).map(|c| json!({"self": s(c, "self"), "id": s(c, "id"), "name": s(c, "name"), "description": s(c, "description")}))).collect::<Vec<_>>()));
    let vers = versions(ctx, pkey);
    for k in ["fixVersions", "versions"] {
        out.insert(
            k.into(),
            json!(
                names(k)
                    .iter()
                    .filter_map(|n| vers
                        .iter()
                        .find(|v| s(v, "name") == n)
                        .map(|v| version_obj(ctx, v)))
                    .collect::<Vec<_>>()
            ),
        );
    }
    out.insert("created".into(), json!(s(f, "created")));
    out.insert(
        "updated".into(),
        json!(if s(f, "updated").is_empty() {
            s(f, "created")
        } else {
            s(f, "updated")
        }),
    );
    out.insert(
        "duedate".into(),
        f.get("duedate").cloned().unwrap_or(Value::Null),
    );
    out.insert("lastViewed".into(), Value::Null);
    let comments: Vec<Value> = comments_of(ctx, key)
        .iter()
        .map(|c| comment_obj(ctx, c))
        .collect();
    out.insert("comment".into(), json!({"comments": comments, "maxResults": comments.len(), "total": comments.len(), "startAt": 0}));
    let worklogs: Vec<Value> = ctx
        .all("worklogs")
        .into_iter()
        .filter(|w| s(w, "issue") == key)
        .map(|w| worklog_obj(ctx, &w))
        .collect();
    let spent: i64 = worklogs
        .iter()
        .map(|w| w["timeSpentSeconds"].as_i64().unwrap_or(0))
        .sum();
    out.insert(
        "worklog".into(),
        json!({"startAt": 0, "maxResults": 20, "total": worklogs.len(), "worklogs": worklogs}),
    );
    let orig = f["timeoriginalestimate"].as_i64();
    let remaining = f["timeestimate"]
        .as_i64()
        .or(orig.map(|o| (o - spent).max(0)));
    let mut tt = Map::new();
    if let Some(o) = orig {
        tt.insert("originalEstimate".into(), json!(format_duration(o)));
        tt.insert("originalEstimateSeconds".into(), json!(o));
    }
    if let Some(r) = remaining {
        tt.insert("remainingEstimate".into(), json!(format_duration(r)));
        tt.insert("remainingEstimateSeconds".into(), json!(r));
    }
    if spent > 0 {
        tt.insert("timeSpent".into(), json!(format_duration(spent)));
        tt.insert("timeSpentSeconds".into(), json!(spent));
    }
    out.insert("timetracking".into(), Value::Object(tt));
    out.insert(
        "timeoriginalestimate".into(),
        orig.map_or(Value::Null, |o| json!(o)),
    );
    out.insert(
        "timeestimate".into(),
        remaining.map_or(Value::Null, |o| json!(o)),
    );
    out.insert(
        "timespent".into(),
        if spent > 0 { json!(spent) } else { Value::Null },
    );
    out.insert(
        "aggregatetimespent".into(),
        if spent > 0 { json!(spent) } else { Value::Null },
    );
    out.insert(
        "attachment".into(),
        json!(
            ctx.all("attachments")
                .into_iter()
                .filter(|a| s(a, "issue") == key)
                .map(|a| attachment_obj(ctx, &a))
                .collect::<Vec<_>>()
        ),
    );
    out.insert("issuelinks".into(), json!(links_of(ctx, key)));
    out.insert(
        "subtasks".into(),
        json!(
            ctx.all("issues")
                .into_iter()
                .filter(|x| s(&x["fields"], "parent") == key)
                .map(|x| brief_issue(ctx, s(&x, "key")))
                .collect::<Vec<_>>()
        ),
    );
    if !s(f, "parent").is_empty() {
        out.insert("parent".into(), brief_issue(ctx, s(f, "parent")));
    }
    let watchers = i["watchers"].as_array().map_or(0, Vec::len);
    let me = ctx.me_name();
    out.insert("watches".into(), json!({"self": ctx.api(&format!("issue/{key}/watchers")), "watchCount": watchers, "isWatching": i["watchers"].as_array().is_some_and(|w| w.iter().any(|x| x == me.as_str()))}));
    out.insert(
        "votes".into(),
        json!({"self": ctx.api(&format!("issue/{key}/votes")), "votes": 0, "hasVoted": false}),
    );
    out.insert("security".into(), Value::Null);
    out.insert(
        EPIC_LINK.into(),
        f.get(EPIC_LINK).cloned().unwrap_or(Value::Null),
    );
    out.insert(
        EPIC_NAME.into(),
        f.get(EPIC_NAME).cloned().unwrap_or(Value::Null),
    );
    out.insert(
        STORY_POINTS.into(),
        f.get(STORY_POINTS).cloned().unwrap_or(Value::Null),
    );
    out.insert(RANK.into(), json!(format!("0|i000{}:", s(i, "id"))));
    let sprints: Vec<Value> = f[SPRINT]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|x| sprint_doc(ctx, x))
        .map(|sp| json!(sprint_string(&sp)))
        .collect();
    out.insert(
        SPRINT.into(),
        if sprints.is_empty() {
            Value::Null
        } else {
            json!(sprints)
        },
    );
    // Other custom fields pass through.
    if let Some(obj) = f.as_object() {
        for (k, v) in obj {
            if k.starts_with("customfield_") && !out.contains_key(k) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

fn wanted_fields(spec: Option<&str>) -> (Option<HashSet<String>>, HashSet<String>) {
    let spec = spec.unwrap_or("*all");
    let mut include = HashSet::new();
    let mut exclude = HashSet::new();
    let mut all = false;
    for p in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if p == "*all" || p == "*navigable" {
            all = true;
        } else if let Some(x) = p.strip_prefix('-') {
            exclude.insert(x.to_owned());
        } else {
            include.insert(p.to_owned());
        }
    }
    (if all { None } else { Some(include) }, exclude)
}

pub fn render_issue(ctx: &Ctx, i: &Value, fields: Option<&str>, expand: Option<&str>) -> Value {
    let all = render_fields(ctx, i);
    let (inc, exc) = wanted_fields(fields);
    let mut out_fields = Map::new();
    for (k, v) in &all {
        let keep = inc
            .as_ref()
            .is_none_or(|set| set.contains(k) || set.iter().any(|x| x.eq_ignore_ascii_case(k)));
        if keep && !exc.contains(k) {
            out_fields.insert(k.clone(), v.clone());
        }
    }
    if let Some(set) = &inc {
        // Fields requested by name that the issue doesn't carry come back absent.
        for want in set {
            if !out_fields.contains_key(want)
                && let Some(id) = field_id_for(ctx, want)
                && let Some(v) = all.get(&id)
            {
                out_fields.insert(id, v.clone());
            }
        }
    }
    let mut o = json!({"expand": "renderedFields,names,schema,operations,editmeta,changelog,versionedRepresentations", "id": s(i, "id"), "self": ctx.api(&format!("issue/{}", s(i, "id"))), "key": s(i, "key"), "fields": out_fields});
    let ex = expand.unwrap_or("");
    if ex.contains("renderedFields") {
        let mut r = Map::new();
        for (k, v) in o["fields"].as_object().cloned().unwrap_or_default() {
            r.insert(
                k.clone(),
                match (k.as_str(), &v) {
                    ("description" | "environment", Value::String(t)) => json!(format!(
                        "<p>{}</p>",
                        html_escape(t)
                            .replace("\n\n", "</p>\n\n<p>")
                            .replace('\n', "<br/>\n")
                    )),
                    ("created" | "updated" | "resolutiondate", Value::String(t)) => {
                        json!(parse_time(t).map(user_date).unwrap_or_default())
                    }
                    _ => Value::Null,
                },
            );
        }
        o["renderedFields"] = Value::Object(r);
    }
    if ex.contains("changelog") {
        let hist: Vec<Value> = i["changelog"].as_array().cloned().unwrap_or_default().iter().map(|h| json!({"id": s(h, "id"), "author": ctx.user(s(h, "author")), "created": s(h, "created"), "items": h["items"]})).collect();
        o["changelog"] =
            json!({"startAt": 0, "maxResults": hist.len(), "total": hist.len(), "histories": hist});
    }
    if ex.contains("names") {
        let names: Map<String, Value> = all_fields(ctx)
            .iter()
            .map(|f| (s(f, "id").to_owned(), json!(s(f, "name"))))
            .collect();
        o["names"] = Value::Object(names);
    }
    if ex.contains("transitions") {
        o["transitions"] = json!(available_transitions(ctx, i, false));
    }
    if let Some(props) = i.get("properties") {
        o["properties"] = props.clone();
    }
    o
}

fn html_escape(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ───────────────────────────── search ─────────────────────────────

fn priority_rank(ctx: &Ctx, name: &str) -> i64 {
    find_named(ctx, "priorities", name)
        .and_then(|p| id_num(&p["id"]))
        .unwrap_or(99)
}

fn key_parts(key: &str) -> (String, i64) {
    let (p, n) = key.rsplit_once('-').unwrap_or((key, "0"));
    (p.to_owned(), n.parse().unwrap_or(0))
}

fn field_values(ctx: &Ctx, i: &Value, field: &str) -> Option<Vec<String>> {
    let f = &i["fields"];
    let key = s(i, "key");
    let str_list = |k: &str| -> Vec<String> {
        f[k].as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let one = |k: &str| -> Vec<String> {
        f[k].as_str()
            .filter(|x| !x.is_empty())
            .map(|x| vec![x.to_owned()])
            .unwrap_or_default()
    };
    let user = |k: &str| -> Vec<String> {
        match f[k].as_str().and_then(|n| ctx.user_doc(n)) {
            Some(u) => vec![
                s(&u, "name").to_owned(),
                s(&u, "displayName").to_owned(),
                s(&u, "emailAddress").to_owned(),
                s(&u, "key").to_owned(),
            ],
            None => vec![],
        }
    };
    let named = |coll: &str, k: &str| -> Vec<String> {
        let n = s(f, k);
        if n.is_empty() {
            return vec![];
        }
        let mut v = vec![n.to_owned()];
        if let Some(d) = find_named(ctx, coll, n) {
            v.push(s(&d, "id").to_owned());
        }
        v
    };
    Some(match field {
        "project" => {
            let p = project_doc(ctx, s(f, "project"));
            p.map(|p| {
                vec![
                    s(&p, "key").to_owned(),
                    s(&p, "id").to_owned(),
                    s(&p, "name").to_owned(),
                ]
            })
            .unwrap_or_default()
        }
        "key" | "issuekey" | "issue" | "id" => vec![key.to_owned(), s(i, "id").to_owned()],
        "status" => named("statuses", "status"),
        "statuscategory" => {
            let cat = find_named(ctx, "statuses", s(f, "status"))
                .map(|x| s(&x, "statusCategory").to_owned())
                .unwrap_or_default();
            let c = status_category(&cat);
            vec![
                s(&c, "name").to_owned(),
                s(&c, "key").to_owned(),
                c["id"].to_string(),
            ]
        }
        "assignee" => user("assignee"),
        "reporter" => user("reporter"),
        "creator" => user(if f["creator"].is_null() {
            "reporter"
        } else {
            "creator"
        }),
        "priority" => {
            let mut v = named("priorities", "priority");
            if v.len() > 1 {
                // Ordering comparisons use the rank: lower id is more urgent.
                v.push(priority_rank(ctx, s(f, "priority")).to_string());
            }
            v
        }
        "issuetype" | "type" => named("issuetypes", "issuetype"),
        "labels" => str_list("labels"),
        "summary" => one("summary"),
        "description" => one("description"),
        "environment" => one("environment"),
        "comment" => comments_of(ctx, key)
            .iter()
            .map(|c| s(c, "body").to_owned())
            .collect(),
        "text" => {
            let mut v = one("summary");
            v.extend(one("description"));
            v.extend(one("environment"));
            v.extend(
                comments_of(ctx, key)
                    .iter()
                    .map(|c| s(c, "body").to_owned()),
            );
            v
        }
        "created" | "createddate" => one("created"),
        "updated" | "updateddate" => one(if s(f, "updated").is_empty() {
            "created"
        } else {
            "updated"
        }),
        "duedate" | "due" => one("duedate"),
        "resolved" | "resolutiondate" => one("resolutiondate"),
        "resolution" => {
            let r = one("resolution");
            if r.is_empty() { vec![] } else { r }
        }
        "component" => str_list("components"),
        "fixversion" => str_list("fixVersions"),
        "affectedversion" => str_list("versions"),
        "sprint" | "customfield_10104" => f[SPRINT]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|x| sprint_doc(ctx, x))
            .flat_map(|sp| {
                vec![
                    s(&sp, "id").to_owned(),
                    sp["id"].to_string(),
                    s(&sp, "name").to_owned(),
                ]
            })
            .collect(),
        "parent" => one("parent"),
        "epic link" | "customfield_10100" => {
            let mut v = one(EPIC_LINK);
            if v.is_empty() {
                v = one("parent")
                    .into_iter()
                    .filter(|p| {
                        issue_doc(ctx, p).is_some_and(|e| s(&e["fields"], "issuetype") == "Epic")
                    })
                    .collect();
            }
            v
        }
        "epic name" | "customfield_10102" => one(EPIC_NAME),
        "story points" | "customfield_10106" => f[STORY_POINTS]
            .as_f64()
            .map(|x| vec![x.to_string()])
            .unwrap_or_default(),
        "issuelinktype" => ctx
            .all("issuelinks")
            .into_iter()
            .filter_map(|l| {
                let t = ctx.get("issuelinktypes", s(&l, "type"))?;
                if s(&l, "inward") == key {
                    Some(vec![s(&t, "inward").to_owned(), s(&t, "name").to_owned()])
                } else if s(&l, "outward") == key {
                    Some(vec![s(&t, "outward").to_owned(), s(&t, "name").to_owned()])
                } else {
                    None
                }
            })
            .flatten()
            .collect(),
        "issue in linkedissues" => vec![],
        "watcher" | "watchers" => i["watchers"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        "level" | "lastviewed" | "votes" => vec![],
        other => {
            let id = if other.starts_with("customfield_") {
                other.to_owned()
            } else {
                field_id_for(ctx, other)?
            };
            match &f[&id] {
                Value::Null => vec![],
                Value::String(x) => vec![x.clone()],
                Value::Array(a) => a
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| x.to_string())
                    })
                    .collect(),
                Value::Object(o) => o
                    .get("value")
                    .or(o.get("name"))
                    .map(|x| vec![x.as_str().unwrap_or("").to_owned()])
                    .unwrap_or_default(),
                v => vec![v.to_string()],
            }
        }
    })
}

pub fn run_jql(ctx: &Ctx, q: &str) -> Result<Vec<Value>, String> {
    let parsed = jql::parse(q)?;
    let mut projects = Vec::new();
    jql::named_values(&parsed.expr, "project", &mut projects);
    for p in &projects {
        if project_doc(ctx, p).is_none() {
            return Err(format!(
                "The value '{p}' does not exist for the field 'project'."
            ));
        }
    }
    let mut keys = Vec::new();
    jql::named_values(&parsed.expr, "key", &mut keys);
    jql::named_values(&parsed.expr, "issuekey", &mut keys);
    for k in &keys {
        if issue_doc(ctx, k).is_none() && k.contains('-') {
            let (p, _) = key_parts(k);
            if project_doc(ctx, &p).is_none() {
                return Err(format!("The issue key '{k}' for field 'key' is invalid."));
            }
            return Err(format!(
                "An issue with key '{k}' does not exist for field 'key'."
            ));
        }
    }
    let me = ctx.me_name();
    let sprints = |state: &str| -> Vec<String> {
        ctx.all("sprints")
            .into_iter()
            .filter(|sp| s(sp, "state") == state)
            .map(|sp| s(&sp, "id").to_owned())
            .collect()
    };
    let env = jql::Env {
        now: ctx.now,
        me: &me,
        sprints: &sprints,
    };
    let mut out = Vec::new();
    for i in ctx.all("issues") {
        let get = |f: &str| field_values(ctx, &i, f);
        if jql::eval(&parsed.expr, &env, &get)? {
            out.push(i);
        }
    }
    let order = if parsed.order.is_empty() {
        vec![("key".to_owned(), true)]
    } else {
        parsed.order
    };
    out.sort_by(|a, b| {
        for (field, desc) in &order {
            let ord = match field.as_str() {
                "key" | "issuekey" | "id" => {
                    let (pa, na) = key_parts(s(a, "key"));
                    let (pb, nb) = key_parts(s(b, "key"));
                    pa.cmp(&pb).then(na.cmp(&nb))
                }
                "priority" => priority_rank(ctx, s(&b["fields"], "priority"))
                    .cmp(&priority_rank(ctx, s(&a["fields"], "priority"))),
                "rank" => id_num(&a["id"]).cmp(&id_num(&b["id"])),
                "created" | "updated" | "duedate" | "resolved" => {
                    let k = if field == "resolved" {
                        "resolutiondate"
                    } else {
                        field.as_str()
                    };
                    s(&a["fields"], k).cmp(s(&b["fields"], k))
                }
                other => field_values(ctx, a, other)
                    .unwrap_or_default()
                    .first()
                    .cloned()
                    .unwrap_or_default()
                    .cmp(
                        &field_values(ctx, b, other)
                            .unwrap_or_default()
                            .first()
                            .cloned()
                            .unwrap_or_default(),
                    ),
            };
            let ord = if *desc { ord.reverse() } else { ord };
            if ord.is_ne() {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
    Ok(out)
}

fn search(ctx: &Ctx, req: &Req) -> Out {
    let b = &req.body;
    let jql_text = req
        .q("jql")
        .map(str::to_owned)
        .or_else(|| b["jql"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let start = req
        .q_i64("startAt")
        .or(b["startAt"].as_i64())
        .unwrap_or(0)
        .max(0) as usize;
    let max = req
        .q_i64("maxResults")
        .or(b["maxResults"].as_i64())
        .unwrap_or(50)
        .clamp(0, 1000) as usize;
    let fields = req
        .q("fields")
        .map(str::to_owned)
        .or_else(|| {
            b["fields"].as_array().map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            })
        })
        .unwrap_or_else(|| "*navigable".into());
    let expand = req.q("expand").map(str::to_owned).or_else(|| {
        b["expand"].as_str().map(str::to_owned).or_else(|| {
            b["expand"].as_array().map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            })
        })
    });
    let found = match run_jql(ctx, &jql_text) {
        Ok(f) => f,
        Err(e) => return error(400, &e),
    };
    let total = found.len();
    let issues: Vec<Value> = found
        .iter()
        .skip(start)
        .take(max)
        .map(|i| render_issue(ctx, i, Some(&fields), expand.as_deref()))
        .collect();
    let mut out = json!({"expand": "schema,names", "startAt": start, "maxResults": max, "total": total, "issues": issues});
    if expand.as_deref().is_some_and(|e| e.contains("names")) {
        let names: Map<String, Value> = all_fields(ctx)
            .iter()
            .map(|f| (s(f, "id").to_owned(), json!(s(f, "name"))))
            .collect();
        out["names"] = Value::Object(names);
    }
    ok(out)
}

// ───────────────────────────── create / edit ─────────────────────────────

fn ref_name(v: &Value) -> Option<String> {
    match v {
        Value::String(x) => Some(x.clone()),
        Value::Object(o) => o
            .get("name")
            .or(o.get("key"))
            .or(o.get("value"))
            .or(o.get("id"))
            .and_then(|x| {
                x.as_str()
                    .map(str::to_owned)
                    .or_else(|| x.as_i64().map(|n| n.to_string()))
            }),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn history(ctx: &Ctx, i: &mut Value, items: Vec<Value>) {
    if items.is_empty() {
        return;
    }
    let mut ch = i["changelog"].as_array().cloned().unwrap_or_default();
    let base = ctx
        .all("issues")
        .iter()
        .map(|x| x["changelog"].as_array().map_or(0, Vec::len))
        .sum::<usize>() as i64;
    ch.push(json!({"id": (20000 + base).to_string(), "author": ctx.me_name(), "created": ctx.stamp(), "items": items}));
    i["changelog"] = json!(ch);
}

fn item(field: &str, from: Option<String>, to: Option<String>) -> Value {
    json!({"field": field, "fieldtype": "jira", "from": from.clone(), "fromString": from, "to": to.clone(), "toString": to})
}

/// Applies `fields` to the stored issue fields. Errors are Jira's per-field messages.
fn apply_fields(
    ctx: &Ctx,
    stored: &mut Map<String, Value>,
    project: &str,
    input: &Map<String, Value>,
    creating: bool,
    errors: &mut Map<String, Value>,
    changes: &mut Vec<Value>,
) {
    let pdoc = project_doc(ctx, project);
    for (k, v) in input {
        let id = if k.starts_with("customfield_") || stored.contains_key(k) {
            k.clone()
        } else {
            field_id_for(ctx, k).unwrap_or_else(|| k.clone())
        };
        let old = stored.get(&id).cloned().unwrap_or(Value::Null);
        let new = match id.as_str() {
            "project" | "issuekey" => continue,
            "summary" => {
                let t = v.as_str().unwrap_or("").trim().to_owned();
                if t.is_empty() {
                    errors.insert(
                        "summary".into(),
                        json!("You must specify a summary of the issue."),
                    );
                    continue;
                }
                if t.chars().count() > 255 {
                    errors.insert(
                        "summary".into(),
                        json!("Summary must be less than 255 characters."),
                    );
                    continue;
                }
                json!(t)
            }
            "description" | "environment" => match v {
                Value::Null => Value::Null,
                Value::String(x) => json!(x),
                other => json!(other.to_string()),
            },
            "issuetype" => {
                let Some(t) = ref_name(v).and_then(|n| find_named(ctx, "issuetypes", &n)) else {
                    errors.insert("issuetype".into(), json!("valid issue type is required"));
                    continue;
                };
                if let Some(p) = &pdoc
                    && !project_types(ctx, p)
                        .iter()
                        .any(|x| s(x, "id") == s(&t, "id"))
                {
                    errors.insert(
                        "issuetype".into(),
                        json!("The issue type selected is invalid."),
                    );
                    continue;
                }
                json!(s(&t, "name"))
            }
            "priority" => match ref_name(v) {
                None => Value::Null,
                Some(n) => match find_named(ctx, "priorities", &n) {
                    Some(p) => json!(s(&p, "name")),
                    None => {
                        errors.insert(
                            "priority".into(),
                            json!(if v.get("id").is_some() {
                                format!("Priority with id '{n}' does not exist")
                            } else {
                                format!("Priority name '{n}' is not valid")
                            }),
                        );
                        continue;
                    }
                },
            },
            "assignee" | "reporter" => {
                if v.is_null() || ref_name(v).is_some_and(|n| n.is_empty()) {
                    if id == "reporter" {
                        errors.insert("reporter".into(), json!("Reporter is required."));
                        continue;
                    }
                    Value::Null
                } else if ref_name(v).as_deref() == Some("-1") {
                    pdoc.as_ref()
                        .map(|p| json!(s(p, "lead")))
                        .unwrap_or(Value::Null)
                } else {
                    match ctx.resolve_user(v) {
                        Some(n) => json!(n),
                        None => {
                            errors.insert(
                                id.clone(),
                                json!(format!(
                                    "User '{}' does not exist.",
                                    ref_name(v).unwrap_or_default()
                                )),
                            );
                            continue;
                        }
                    }
                }
            }
            "labels" => {
                let labels: Vec<String> = match v {
                    Value::Array(a) => a
                        .iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect(),
                    Value::String(x) => x
                        .split([',', ' '])
                        .filter(|p| !p.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    _ => vec![],
                };
                if let Some(bad) = labels.iter().find(|l| l.contains(' ')) {
                    errors.insert(
                        "labels".into(),
                        json!(format!(
                            "The label '{bad}' contains spaces which is invalid."
                        )),
                    );
                    continue;
                }
                json!(labels)
            }
            "components" => {
                let comps = components(ctx, project);
                let mut names = Vec::new();
                for c in v.as_array().cloned().unwrap_or_default() {
                    let n = ref_name(&c).unwrap_or_default();
                    match comps
                        .iter()
                        .find(|x| s(x, "name").eq_ignore_ascii_case(&n) || s(x, "id") == n)
                    {
                        Some(x) => names.push(json!(s(x, "name"))),
                        None => {
                            errors.insert(
                                "components".into(),
                                json!(format!("Component name '{n}' is not valid")),
                            );
                        }
                    }
                }
                json!(names)
            }
            "fixVersions" | "versions" => {
                let vers = versions(ctx, project);
                let mut names = Vec::new();
                for c in v.as_array().cloned().unwrap_or_default() {
                    let n = ref_name(&c).unwrap_or_default();
                    match vers.iter().find(|x| s(x, "name") == n || s(x, "id") == n) {
                        Some(x) => names.push(json!(s(x, "name"))),
                        None => {
                            errors.insert(
                                id.clone(),
                                json!(format!("Version name '{n}' is not valid")),
                            );
                        }
                    }
                }
                json!(names)
            }
            "duedate" => match v.as_str() {
                None | Some("") => Value::Null,
                Some(d) => match parse_time(d) {
                    Some(t) => json!(stamp(t)[..10].to_owned()),
                    None => {
                        errors.insert(
                            "duedate".into(),
                            json!("Error parsing date string: ".to_owned() + d),
                        );
                        continue;
                    }
                },
            },
            "parent" => match ref_name(v).and_then(|k| issue_doc(ctx, &k)) {
                Some(p) => json!(s(&p, "key")),
                None => {
                    errors.insert("parent".into(), json!("Could not find issue by id or key."));
                    continue;
                }
            },
            "resolution" => match ref_name(v) {
                None => Value::Null,
                Some(n) => match find_named(ctx, "resolutions", &n) {
                    Some(r) => json!(s(&r, "name")),
                    None => {
                        errors.insert(
                            "resolution".into(),
                            json!(format!("Resolution name '{n}' is not valid")),
                        );
                        continue;
                    }
                },
            },
            "timetracking" => {
                if let Some(o) = v
                    .get("originalEstimate")
                    .and_then(Value::as_str)
                    .and_then(crate::model::duration_secs)
                {
                    stored.insert("timeoriginalestimate".into(), json!(o));
                }
                if let Some(r) = v
                    .get("remainingEstimate")
                    .and_then(Value::as_str)
                    .and_then(crate::model::duration_secs)
                {
                    stored.insert("timeestimate".into(), json!(r));
                }
                continue;
            }
            EPIC_LINK => match v.as_str() {
                None | Some("") => Value::Null,
                Some(k) => match issue_doc(ctx, k) {
                    Some(e) if s(&e["fields"], "issuetype") == "Epic" => json!(s(&e, "key")),
                    _ => {
                        errors.insert(
                            EPIC_LINK.into(),
                            json!(format!("The issue {k} is not an Epic.")),
                        );
                        continue;
                    }
                },
            },
            SPRINT => match v {
                Value::Null => Value::Null,
                Value::Array(a) => json!(a.iter().filter_map(id_num).collect::<Vec<_>>()),
                other => match id_num(other) {
                    Some(n) if ctx.get("sprints", &n.to_string()).is_some() => json!([n]),
                    _ => {
                        errors.insert(
                            SPRINT.into(),
                            json!(format!("Sprint with id '{other}' does not exist.")),
                        );
                        continue;
                    }
                },
            },
            other if other.starts_with("customfield_") => {
                if !all_fields(ctx).iter().any(|f| s(f, "id") == other) {
                    errors.insert(other.to_owned(), json!(format!("Field '{other}' cannot be set. It is not on the appropriate screen, or unknown.")));
                    continue;
                }
                v.clone()
            }
            other => {
                if !all_fields(ctx).iter().any(|f| s(f, "id") == other) {
                    errors.insert(other.to_owned(), json!(format!("Field '{other}' cannot be set. It is not on the appropriate screen, or unknown.")));
                    continue;
                }
                v.clone()
            }
        };
        if !creating && old != new {
            let show = |x: &Value| match x {
                Value::Null => None,
                Value::String(t) => Some(t.clone()),
                Value::Array(a) => Some(
                    a.iter()
                        .map(|y| {
                            y.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| y.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                other => Some(other.to_string()),
            };
            changes.push(item(&id, show(&old), show(&new)));
        }
        stored.insert(id, new);
    }
}

/// Applies the `update` verbs (`set`, `add`, `remove`, `edit`).
fn apply_update(
    ctx: &mut Ctx,
    key: &str,
    stored: &mut Map<String, Value>,
    project: &str,
    update: &Map<String, Value>,
    errors: &mut Map<String, Value>,
    changes: &mut Vec<Value>,
) {
    for (field, ops) in update {
        for op in ops.as_array().cloned().unwrap_or_default() {
            let Some((verb, val)) = op
                .as_object()
                .and_then(|o| o.iter().next())
                .map(|(k, v)| (k.clone(), v.clone()))
            else {
                continue;
            };
            match (field.as_str(), verb.as_str()) {
                ("comment", "add") => {
                    let id = ctx.next_id("comments", 10000);
                    let c = json!({"id": id, "issue": key, "author": ctx.me_name(), "body": s(&val, "body"), "created": ctx.stamp(), "updated": ctx.stamp()});
                    ctx.put("comments", &id, c);
                }
                ("issuelinks", "add") => {
                    let mut body = val.clone();
                    if body.get("inwardIssue").is_none() {
                        body["inwardIssue"] = json!({"key": key});
                    }
                    if body.get("outwardIssue").is_none() {
                        body["outwardIssue"] = json!({"key": key});
                    }
                    let _ = insert_link(ctx, &body);
                }
                ("worklog", "add") => {
                    let _ = add_worklog(ctx, key, &val, None);
                }
                ("labels" | "components" | "fixVersions" | "versions", "add" | "remove") => {
                    let mut cur: Vec<Value> = stored
                        .get(field)
                        .and_then(|v| v.as_array().cloned())
                        .unwrap_or_default();
                    let before = cur.clone();
                    let mut probe = Map::new();
                    let item_val = if field == "labels" {
                        val.clone()
                    } else {
                        json!([val.clone()])
                    };
                    probe.insert(
                        field.clone(),
                        if field == "labels" {
                            json!([item_val])
                        } else {
                            item_val
                        },
                    );
                    let mut tmp = Map::new();
                    apply_fields(
                        ctx,
                        &mut tmp,
                        project,
                        &probe,
                        true,
                        errors,
                        &mut Vec::new(),
                    );
                    for x in tmp
                        .get(field)
                        .and_then(|v| v.as_array().cloned())
                        .unwrap_or_default()
                    {
                        if verb == "add" {
                            if !cur.contains(&x) {
                                cur.push(x);
                            }
                        } else {
                            cur.retain(|c| c != &x);
                        }
                    }
                    if cur != before {
                        changes.push(item(
                            field,
                            Some(
                                before
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            ),
                            Some(
                                cur.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            ),
                        ));
                    }
                    stored.insert(field.clone(), json!(cur));
                }
                (_, "set") => {
                    let mut m = Map::new();
                    m.insert(field.clone(), val);
                    apply_fields(ctx, stored, project, &m, false, errors, changes);
                }
                _ => {}
            }
        }
    }
}

pub fn next_issue_key(ctx: &Ctx, project: &str) -> String {
    let n = ctx
        .all("issues")
        .iter()
        .filter(|i| s(&i["fields"], "project") == project)
        .map(|i| key_parts(s(i, "key")).1)
        .max()
        .unwrap_or(0);
    format!("{project}-{}", n + 1)
}

/// Creates an issue from a create body. Returns (key, response) or the error response.
pub fn create_issue(ctx: &mut Ctx, body: &Value) -> Result<(String, Value), Out> {
    let input = body["fields"].as_object().cloned().unwrap_or_default();
    let mut errors = Map::new();
    let project = match input
        .get("project")
        .and_then(ref_name)
        .and_then(|p| project_doc(ctx, &p))
    {
        Some(p) => s(&p, "key").to_owned(),
        None => {
            errors.insert(
                "project".into(),
                json!(if input.contains_key("project") {
                    "valid project is required"
                } else {
                    "project is required"
                }),
            );
            String::new()
        }
    };
    if !input.contains_key("issuetype") {
        errors.insert("issuetype".into(), json!("issue type is required"));
    }
    if !input.contains_key("summary") {
        errors.insert(
            "summary".into(),
            json!("You must specify a summary of the issue."),
        );
    }
    if !errors.is_empty() && project.is_empty() {
        return Err(field_errors(Value::Object(errors)));
    }
    let now = ctx.stamp();
    let me = ctx.me_name();
    let mut stored = Map::new();
    stored.insert("project".into(), json!(project));
    stored.insert("status".into(), json!(initial_status(ctx)));
    stored.insert(
        "priority".into(),
        json!(
            find_named(ctx, "priorities", "Medium")
                .map(|p| s(&p, "name").to_owned())
                .unwrap_or_else(|| "Medium".into())
        ),
    );
    stored.insert("reporter".into(), json!(me));
    stored.insert("creator".into(), json!(me));
    stored.insert("assignee".into(), Value::Null);
    stored.insert("labels".into(), json!([]));
    stored.insert("components".into(), json!([]));
    stored.insert("fixVersions".into(), json!([]));
    stored.insert("versions".into(), json!([]));
    stored.insert("description".into(), Value::Null);
    stored.insert("duedate".into(), Value::Null);
    stored.insert("resolution".into(), Value::Null);
    stored.insert("resolutiondate".into(), Value::Null);
    stored.insert("created".into(), json!(now));
    stored.insert("updated".into(), json!(now));
    apply_fields(
        ctx,
        &mut stored,
        &project,
        &input,
        true,
        &mut errors,
        &mut Vec::new(),
    );
    if s(&Value::Object(stored.clone()), "issuetype").is_empty()
        && !errors.contains_key("issuetype")
    {
        errors.insert("issuetype".into(), json!("issue type is required"));
    }
    let is_sub = find_named(
        ctx,
        "issuetypes",
        stored
            .get("issuetype")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )
    .is_some_and(|t| t["subtask"].as_bool().unwrap_or(false));
    if is_sub && stored.get("parent").is_none_or(Value::is_null) {
        errors.insert(
            "parent".into(),
            json!("Issue type is a sub-task but parent issue key or id not specified."),
        );
    }
    if !errors.is_empty() {
        return Err(field_errors(Value::Object(errors)));
    }
    let key = next_issue_key(ctx, &project);
    let id = ctx.next_id("issues", 10000);
    let mut doc =
        json!({"id": id, "key": key, "fields": stored, "watchers": [me], "changelog": []});
    if let Some(update) = body["update"].as_object().cloned() {
        let mut stored2 = doc["fields"].as_object().cloned().unwrap_or_default();
        ctx.put("issues", &key, doc.clone());
        apply_update(
            ctx,
            &key,
            &mut stored2,
            &project,
            &update,
            &mut errors,
            &mut Vec::new(),
        );
        doc["fields"] = Value::Object(stored2);
    }
    ctx.put("issues", &key, doc);
    let resp = json!({"id": id, "key": key, "self": ctx.api(&format!("issue/{id}"))});
    Ok((key, resp))
}

fn initial_status(ctx: &Ctx) -> String {
    ctx.all("statuses")
        .into_iter()
        .find(|x| x["initial"].as_bool().unwrap_or(false))
        .or_else(|| ctx.all("statuses").into_iter().next())
        .map(|x| s(&x, "name").to_owned())
        .unwrap_or_else(|| "To Do".into())
}

fn bulk_create(ctx: &mut Ctx, req: &Req) -> Out {
    let updates = req.body["issueUpdates"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut issues = Vec::new();
    let mut errors = Vec::new();
    for (i, u) in updates.iter().enumerate() {
        match create_issue(ctx, u) {
            Ok((_, v)) => issues.push(v),
            Err(Out::Json(status, body)) => errors.push(
                json!({"status": status.as_u16(), "elementErrors": body, "failedElementNumber": i}),
            ),
            Err(_) => {}
        }
    }
    let status = if issues.is_empty() && !errors.is_empty() {
        400
    } else {
        201
    };
    Out::Json(
        axum::http::StatusCode::from_u16(status).expect("status"),
        json!({"issues": issues, "errors": errors}),
    )
}

fn edit_issue(ctx: &mut Ctx, key: &str, body: &Value) -> Out {
    let Some(mut doc) = issue_doc(ctx, key) else {
        return issue_missing();
    };
    let key = s(&doc, "key").to_owned();
    let project = s(&doc["fields"], "project").to_owned();
    let mut stored = doc["fields"].as_object().cloned().unwrap_or_default();
    let mut errors = Map::new();
    let mut changes = Vec::new();
    if let Some(f) = body["fields"].as_object() {
        if f.contains_key("project") {
            errors.insert("project".into(), json!("Field 'project' cannot be set. It is not on the appropriate screen, or unknown."));
        }
        apply_fields(
            ctx,
            &mut stored,
            &project,
            f,
            false,
            &mut errors,
            &mut changes,
        );
    }
    if !errors.is_empty() {
        return field_errors(Value::Object(errors));
    }
    ctx.put("issues", &key, doc.clone());
    if let Some(u) = body["update"].as_object() {
        apply_update(
            ctx,
            &key,
            &mut stored,
            &project,
            u,
            &mut errors,
            &mut changes,
        );
        if !errors.is_empty() {
            return field_errors(Value::Object(errors));
        }
    }
    stored.insert("updated".into(), json!(ctx.stamp()));
    doc = issue_doc(ctx, &key).unwrap_or(doc);
    doc["fields"] = Value::Object(stored);
    history(ctx, &mut doc, changes);
    ctx.put("issues", &key, doc);
    no_content()
}

fn touch(ctx: &mut Ctx, key: &str, items: Vec<Value>) {
    if let Some(mut doc) = issue_doc(ctx, key) {
        doc["fields"]["updated"] = json!(ctx.stamp());
        history(ctx, &mut doc, items);
        ctx.put("issues", s(&doc, "key").to_owned().as_str(), doc);
    }
}

// ───────────────────────────── issue sub-resources ─────────────────────────────

fn issue_route(ctx: &mut Ctx, req: &Req, m: &str, key: &str, tail: &[&str]) -> Out {
    let Some(doc) = issue_doc(ctx, key) else {
        return issue_missing();
    };
    let key = s(&doc, "key").to_owned();
    match (m, tail) {
        ("GET", []) => ok(render_issue(ctx, &doc, req.q("fields"), req.q("expand"))),
        ("PUT", []) => edit_issue(ctx, &key, &req.body),
        ("DELETE", []) => {
            let subs: Vec<Value> = ctx
                .all("issues")
                .into_iter()
                .filter(|i| s(&i["fields"], "parent") == key)
                .collect();
            if !subs.is_empty() && req.q("deleteSubtasks") != Some("true") {
                return error(
                    400,
                    &format!(
                        "The issue '{key}' has subtasks.  You must specify the 'deleteSubtasks' parameter to delete this issue and all its subtasks."
                    ),
                );
            }
            for sub in subs {
                ctx.remove("issues", s(&sub, "key"));
            }
            ctx.remove("issues", &key);
            for coll in ["comments", "worklogs", "attachments", "remotelinks"] {
                for d in ctx.all(coll).into_iter().filter(|d| s(d, "issue") == key) {
                    ctx.remove(coll, s(&d, "id"));
                }
            }
            for l in ctx
                .all("issuelinks")
                .into_iter()
                .filter(|l| s(l, "inward") == key || s(l, "outward") == key)
            {
                ctx.remove("issuelinks", s(&l, "id"));
            }
            no_content()
        }
        ("GET", ["editmeta"]) => {
            let t =
                find_named(ctx, "issuetypes", s(&doc["fields"], "issuetype")).unwrap_or(json!({}));
            let fields = meta_fields(ctx, s(&doc["fields"], "project"), &t, false);
            ok(json!({"fields": fields}))
        }
        ("GET", ["comment"]) => {
            let all: Vec<Value> = comments_of(ctx, &key)
                .iter()
                .map(|c| comment_obj(ctx, c))
                .collect();
            let start = req.q_i64("startAt").unwrap_or(0).max(0) as usize;
            let max = req.q_i64("maxResults").unwrap_or(5000).max(0) as usize;
            let total = all.len();
            let page: Vec<Value> = all.into_iter().skip(start).take(max).collect();
            ok(
                json!({"startAt": start, "maxResults": max.min(total.max(page.len())), "total": total, "comments": page}),
            )
        }
        ("POST", ["comment"]) => {
            let body = s(&req.body, "body").to_owned();
            if body.trim().is_empty() {
                return field_errors(json!({"comment": "Comment body can not be empty!"}));
            }
            let id = ctx.next_id("comments", 10000);
            let mut c = json!({"id": id, "issue": key, "author": ctx.me_name(), "body": body, "created": ctx.stamp(), "updated": ctx.stamp()});
            if let Some(v) = req.body.get("visibility").filter(|v| !v.is_null()) {
                c["visibility"] = v.clone();
            }
            ctx.put("comments", &id, c.clone());
            touch(ctx, &key, vec![]);
            created(comment_obj(ctx, &c))
        }
        ("GET", ["comment", id]) => {
            match ctx.get("comments", id).filter(|c| s(c, "issue") == key) {
                Some(c) => ok(comment_obj(ctx, &c)),
                None => error(404, &format!("Can not find a comment for the id: {id}.")),
            }
        }
        ("PUT", ["comment", id]) => {
            let Some(mut c) = ctx.get("comments", id).filter(|c| s(c, "issue") == key) else {
                return error(404, &format!("Can not find a comment for the id: {id}."));
            };
            let body = s(&req.body, "body").to_owned();
            if body.trim().is_empty() {
                return field_errors(json!({"comment": "Comment body can not be empty!"}));
            }
            c["body"] = json!(body);
            c["updated"] = json!(ctx.stamp());
            c["updateAuthor"] = json!(ctx.me_name());
            if let Some(v) = req.body.get("visibility") {
                c["visibility"] = v.clone();
            }
            ctx.put("comments", id, c.clone());
            ok(comment_obj(ctx, &c))
        }
        ("DELETE", ["comment", id]) => {
            if ctx
                .get("comments", id)
                .filter(|c| s(c, "issue") == key)
                .is_none()
            {
                return error(404, &format!("Can not find a comment for the id: {id}."));
            }
            ctx.remove("comments", id);
            no_content()
        }
        ("GET", ["transitions"]) => {
            let with_fields = req
                .q("expand")
                .is_some_and(|e| e.contains("transitions.fields"));
            let mut list = available_transitions(ctx, &doc, with_fields);
            if let Some(tid) = req.q("transitionId") {
                list.retain(|t| s(t, "id") == tid);
            }
            ok(json!({"expand": "transitions", "transitions": list}))
        }
        ("POST", ["transitions"]) => transition(ctx, &key, &req.body),
        ("GET", ["worklog"]) => {
            let all: Vec<Value> = ctx
                .all("worklogs")
                .into_iter()
                .filter(|w| s(w, "issue") == key)
                .map(|w| worklog_obj(ctx, &w))
                .collect();
            ok(json!({"startAt": 0, "maxResults": all.len(), "total": all.len(), "worklogs": all}))
        }
        ("POST", ["worklog"]) => match add_worklog(ctx, &key, &req.body, Some(req)) {
            Ok(w) => created(worklog_obj(ctx, &w)),
            Err(e) => e,
        },
        ("GET", ["worklog", id]) => {
            match ctx.get("worklogs", id).filter(|w| s(w, "issue") == key) {
                Some(w) => ok(worklog_obj(ctx, &w)),
                None => error(404, &format!("Cannot find worklog with id: '{id}'.")),
            }
        }
        ("GET", ["watchers"]) => {
            let w: Vec<Value> = doc["watchers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_str)
                .map(|n| ctx.user(n))
                .collect();
            let me = ctx.me_name();
            ok(
                json!({"self": ctx.api(&format!("issue/{key}/watchers")), "isWatching": w.iter().any(|u| s(u, "name") == me), "watchCount": w.len(), "watchers": w}),
            )
        }
        ("POST", ["watchers"]) => {
            let who = match &req.body {
                Value::String(x) => x.clone(),
                Value::Null => String::from_utf8_lossy(&req.raw)
                    .trim()
                    .trim_matches('"')
                    .to_owned(),
                other => ref_name(other).unwrap_or_default(),
            };
            let who = if who.is_empty() { ctx.me_name() } else { who };
            let Some(name) = ctx.user_doc(&who).map(|u| s(&u, "name").to_owned()) else {
                return error(404, &format!("The user \"{who}\" does not exist."));
            };
            let mut d = doc.clone();
            let mut w: Vec<Value> = d["watchers"].as_array().cloned().unwrap_or_default();
            if !w.iter().any(|x| x == name.as_str()) {
                w.push(json!(name));
            }
            d["watchers"] = json!(w);
            ctx.put("issues", &key, d);
            no_content()
        }
        ("DELETE", ["watchers"]) => {
            let who = req
                .q("username")
                .or(req.q("accountId"))
                .unwrap_or("")
                .to_owned();
            let Some(name) = ctx.user_doc(&who).map(|u| s(&u, "name").to_owned()) else {
                return error(404, &format!("The user \"{who}\" does not exist."));
            };
            let mut d = doc.clone();
            let w: Vec<Value> = d["watchers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|x| x != name.as_str())
                .collect();
            d["watchers"] = json!(w);
            ctx.put("issues", &key, d);
            no_content()
        }
        ("PUT", ["assignee"]) => {
            let target = &req.body["name"];
            let new = match target {
                Value::Null => Value::Null,
                Value::String(x) if x == "-1" => json!(
                    project_doc(ctx, s(&doc["fields"], "project"))
                        .map(|p| s(&p, "lead").to_owned())
                        .unwrap_or_default()
                ),
                other => match ctx.resolve_user(other) {
                    Some(n) => json!(n),
                    None => {
                        return error(
                            404,
                            &format!(
                                "User '{}' does not exist.",
                                ref_name(other).unwrap_or_default()
                            ),
                        );
                    }
                },
            };
            let mut d = doc.clone();
            let old = d["fields"]["assignee"].as_str().map(str::to_owned);
            d["fields"]["assignee"] = new.clone();
            d["fields"]["updated"] = json!(ctx.stamp());
            if old.as_deref() != new.as_str() {
                history(
                    ctx,
                    &mut d,
                    vec![item("assignee", old, new.as_str().map(str::to_owned))],
                );
            }
            ctx.put("issues", &key, d);
            no_content()
        }
        ("GET", ["remotelink"]) => ok(json!(
            ctx.all("remotelinks")
                .into_iter()
                .filter(|l| s(l, "issue") == key)
                .map(|l| remote_obj(ctx, &l))
                .collect::<Vec<_>>()
        )),
        ("POST", ["remotelink"]) => {
            let url = s(&req.body["object"], "url").to_owned();
            let title = s(&req.body["object"], "title").to_owned();
            if url.is_empty() || title.is_empty() {
                return field_errors(
                    json!({"title": "'title' is required.", "url": "'url' is required."}),
                );
            }
            let gid = s(&req.body, "globalId").to_owned();
            if let Some(mut existing) = ctx
                .all("remotelinks")
                .into_iter()
                .find(|l| !gid.is_empty() && s(l, "issue") == key && s(l, "globalId") == gid)
            {
                existing["object"] = req.body["object"].clone();
                existing["relationship"] =
                    req.body.get("relationship").cloned().unwrap_or(Value::Null);
                let id = s(&existing, "id").to_owned();
                ctx.put("remotelinks", &id, existing);
                return ok(
                    json!({"id": id_num(&json!(id)), "self": ctx.api(&format!("issue/{key}/remotelink/{id}"))}),
                );
            }
            let id = ctx.next_id("remotelinks", 10000);
            let l = json!({"id": id, "issue": key, "globalId": gid, "application": req.body.get("application").cloned().unwrap_or(json!({})), "relationship": req.body.get("relationship").cloned().unwrap_or(Value::Null), "object": req.body["object"]});
            ctx.put("remotelinks", &id, l);
            created(
                json!({"id": id_num(&json!(id)), "self": ctx.api(&format!("issue/{key}/remotelink/{id}"))}),
            )
        }
        ("DELETE", ["remotelink", id]) => {
            if ctx
                .get("remotelinks", id)
                .filter(|l| s(l, "issue") == key)
                .is_none()
            {
                return error(404, "No remote link with the given id was found.");
            }
            ctx.remove("remotelinks", id);
            no_content()
        }
        ("POST", ["attachments"]) => {
            let files = multipart_files(req);
            if files.is_empty() {
                return error(400, "No attachments were provided.");
            }
            let mut out = Vec::new();
            for (name, data, ct) in files {
                let id = ctx.next_id("attachments", 10000);
                let a = json!({"id": id, "issue": key, "filename": name, "author": ctx.me_name(), "created": ctx.stamp(), "size": data.len(), "mimeType": ct, "content": String::from_utf8_lossy(&data)});
                ctx.put("attachments", &id, a.clone());
                out.push(attachment_obj(ctx, &a));
            }
            ok(json!(out))
        }
        ("GET", ["properties", ..]) => ok(json!({"keys": []})),
        ("GET", ["votes"]) => ok(
            json!({"self": ctx.api(&format!("issue/{key}/votes")), "votes": 0, "hasVoted": false, "voters": []}),
        ),
        _ => not_found_page(),
    }
}

fn remote_obj(ctx: &Ctx, l: &Value) -> Value {
    let mut o = json!({"id": id_num(&l["id"]), "self": ctx.api(&format!("issue/{}/remotelink/{}", s(l, "issue"), s(l, "id"))), "application": l.get("application").cloned().unwrap_or(json!({})), "object": l["object"]});
    if !s(l, "globalId").is_empty() {
        o["globalId"] = json!(s(l, "globalId"));
    }
    if let Some(r) = l.get("relationship").filter(|r| !r.is_null()) {
        o["relationship"] = r.clone();
    }
    o
}

fn multipart_files(req: &Req) -> Vec<(String, Vec<u8>, String)> {
    let Some(boundary) = req
        .content_type
        .split("boundary=")
        .nth(1)
        .map(|b| b.trim_matches('"').to_owned())
    else {
        return vec![];
    };
    let delim = format!("--{boundary}");
    let raw = &req.raw;
    let mut out = Vec::new();
    let positions: Vec<usize> = raw
        .windows(delim.len())
        .enumerate()
        .filter(|(_, w)| *w == delim.as_bytes())
        .map(|(i, _)| i)
        .collect();
    for w in positions.windows(2) {
        let part = &raw[w[0] + delim.len()..w[1]];
        let Some(split) = part.windows(4).position(|x| x == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&part[..split]).to_string();
        let mut data = part[split + 4..].to_vec();
        if data.ends_with(b"\r\n") {
            data.truncate(data.len() - 2);
        }
        let Some(name) = head
            .split("filename=\"")
            .nth(1)
            .and_then(|x| x.split('"').next())
        else {
            continue;
        };
        let ct = head
            .lines()
            .find_map(|l| {
                l.strip_prefix("Content-Type: ")
                    .or(l.strip_prefix("content-type: "))
            })
            .unwrap_or("application/octet-stream")
            .trim()
            .to_owned();
        out.push((name.to_owned(), data, ct));
    }
    out
}

pub fn attachment_content(ctx: &mut Ctx, id: &str) -> Out {
    match ctx.get("attachments", id) {
        Some(a) => Out::Bytes(
            if s(&a, "mimeType").is_empty() {
                "application/octet-stream".into()
            } else {
                s(&a, "mimeType").to_owned()
            },
            s(&a, "content").as_bytes().to_vec(),
        ),
        None => error(
            404,
            &format!("The attachment with id '{id}' does not exist"),
        ),
    }
}

fn add_worklog(ctx: &mut Ctx, key: &str, body: &Value, req: Option<&Req>) -> Result<Value, Out> {
    let secs = body["timeSpentSeconds"].as_i64().or_else(|| {
        body["timeSpent"]
            .as_str()
            .and_then(crate::model::duration_secs)
    });
    let Some(secs) = secs.filter(|x| *x > 0) else {
        return Err(field_errors(
            json!({"timeLogged": "You must indicate the time spent working."}),
        ));
    };
    let started = match body["started"].as_str() {
        Some(st) => match parse_time(st) {
            Some(t) => stamp(t),
            None => {
                return Err(field_errors(
                    json!({"started": "You must specify a valid date for when the work started."}),
                ));
            }
        },
        None => ctx.stamp(),
    };
    let id = ctx.next_id("worklogs", 10000);
    let w = json!({"id": id, "issue": key, "author": ctx.me_name(), "comment": s(body, "comment"), "started": started, "timeSpentSeconds": secs, "created": ctx.stamp()});
    ctx.put("worklogs", &id, w.clone());
    if let Some(mut doc) = issue_doc(ctx, key) {
        let adjust = req.and_then(|r| r.q("adjustEstimate")).unwrap_or("auto");
        let f = &mut doc["fields"];
        let remaining = f["timeestimate"]
            .as_i64()
            .or(f["timeoriginalestimate"].as_i64());
        match (adjust, remaining) {
            ("new", _) => {
                if let Some(n) = req
                    .and_then(|r| r.q("newEstimate"))
                    .and_then(crate::model::duration_secs)
                {
                    f["timeestimate"] = json!(n);
                }
            }
            ("leave", _) => {}
            ("manual", Some(r)) => {
                let by = req
                    .and_then(|r| r.q("reduceBy"))
                    .and_then(crate::model::duration_secs)
                    .unwrap_or(0);
                f["timeestimate"] = json!((r - by).max(0));
            }
            (_, Some(r)) => f["timeestimate"] = json!((r - secs).max(0)),
            _ => {}
        }
        f["updated"] = json!(ctx.stamp());
        ctx.put("issues", key, doc);
    }
    Ok(w)
}

fn available_transitions(ctx: &Ctx, i: &Value, with_fields: bool) -> Vec<Value> {
    let cur = s(&i["fields"], "status");
    let cur_id = find_named(ctx, "statuses", cur)
        .map(|x| s(&x, "id").to_owned())
        .unwrap_or_default();
    ctx.all("transitions")
        .into_iter()
        .filter(|t| {
            let from: Vec<String> = t["from"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
            (from.is_empty() || from.iter().any(|f| f.eq_ignore_ascii_case(cur) || *f == cur_id)) && !s(t, "to").eq_ignore_ascii_case(cur)
        })
        .map(|t| {
            let mut o = json!({"id": s(&t, "id"), "name": s(&t, "name"), "to": status_obj(ctx, s(&t, "to")), "hasScreen": t["screen"].as_bool().unwrap_or(false), "isGlobal": t["from"].as_array().is_none_or(Vec::is_empty), "isInitial": false, "isAvailable": true, "isConditional": false, "isLooped": false});
            if with_fields {
                let mut fields = Map::new();
                if t["screen"].as_bool().unwrap_or(false) && status_done(ctx, s(&t, "to")) {
                    fields.insert("resolution".into(), json!({"required": true, "schema": {"type": "resolution", "system": "resolution"}, "name": "Resolution", "fieldId": "resolution", "operations": ["set"], "allowedValues": ctx.all("resolutions").iter().map(|r| resolution_obj(ctx, s(r, "name"))).collect::<Vec<_>>()}));
                }
                o["fields"] = Value::Object(fields);
            }
            o
        })
        .collect()
}

fn transition(ctx: &mut Ctx, key: &str, body: &Value) -> Out {
    let Some(mut doc) = issue_doc(ctx, key) else {
        return issue_missing();
    };
    let tid = match &body["transition"]["id"] {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => {
            let name = s(&body["transition"], "name");
            if name.is_empty() {
                return error(400, "Missing 'transition' identifier");
            }
            match ctx
                .all("transitions")
                .into_iter()
                .find(|t| s(t, "name").eq_ignore_ascii_case(name))
            {
                Some(t) => s(&t, "id").to_owned(),
                None => {
                    return error(
                        400,
                        &format!("Transition id '{name}' is not valid for this issue."),
                    );
                }
            }
        }
    };
    let avail = available_transitions(ctx, &doc, false);
    let Some(t) = avail.iter().find(|t| s(t, "id") == tid) else {
        return error(
            400,
            &format!("Transition id '{tid}' is not valid for this issue."),
        );
    };
    let to = s(&t["to"], "name").to_owned();
    let from = s(&doc["fields"], "status").to_owned();
    let project = s(&doc["fields"], "project").to_owned();
    let mut stored = doc["fields"].as_object().cloned().unwrap_or_default();
    let mut errors = Map::new();
    let mut changes = vec![item("status", Some(from), Some(to.clone()))];
    if let Some(f) = body["fields"].as_object() {
        apply_fields(
            ctx,
            &mut stored,
            &project,
            f,
            false,
            &mut errors,
            &mut changes,
        );
    }
    if !errors.is_empty() {
        return field_errors(Value::Object(errors));
    }
    stored.insert("status".into(), json!(to));
    if status_done(ctx, &to) {
        if stored.get("resolution").is_none_or(Value::is_null) {
            let r = find_named(ctx, "resolutions", "Done")
                .or_else(|| ctx.all("resolutions").into_iter().next())
                .map(|r| s(&r, "name").to_owned())
                .unwrap_or_else(|| "Done".into());
            changes.push(item("resolution", None, Some(r.clone())));
            stored.insert("resolution".into(), json!(r));
        }
        stored.insert("resolutiondate".into(), json!(ctx.stamp()));
    } else if stored.get("resolution").is_some_and(|r| !r.is_null()) {
        changes.push(item(
            "resolution",
            stored
                .get("resolution")
                .and_then(Value::as_str)
                .map(str::to_owned),
            None,
        ));
        stored.insert("resolution".into(), Value::Null);
        stored.insert("resolutiondate".into(), Value::Null);
    }
    stored.insert("updated".into(), json!(ctx.stamp()));
    doc["fields"] = Value::Object(stored.clone());
    ctx.put("issues", key, doc.clone());
    if let Some(u) = body["update"].as_object() {
        apply_update(
            ctx,
            key,
            &mut stored,
            &project,
            u,
            &mut errors,
            &mut changes,
        );
        doc = issue_doc(ctx, key).unwrap_or(doc);
        doc["fields"] = Value::Object(stored);
    }
    history(ctx, &mut doc, changes);
    ctx.put("issues", key, doc);
    no_content()
}

fn insert_link(ctx: &mut Ctx, body: &Value) -> Result<String, Out> {
    let tname = ref_name(&body["type"]).unwrap_or_default();
    let Some(t) = ctx.all("issuelinktypes").into_iter().find(|t| {
        s(t, "name").eq_ignore_ascii_case(&tname)
            || s(t, "id") == tname
            || s(t, "inward").eq_ignore_ascii_case(&tname)
            || s(t, "outward").eq_ignore_ascii_case(&tname)
    }) else {
        return Err(error(
            404,
            &format!("No issue link type with name '{tname}' found."),
        ));
    };
    let inward = ref_name(&body["inwardIssue"]).and_then(|k| issue_doc(ctx, &k));
    let outward = ref_name(&body["outwardIssue"]).and_then(|k| issue_doc(ctx, &k));
    let (Some(i), Some(o)) = (inward, outward) else {
        return Err(error(404, "Issue Does Not Exist"));
    };
    let id = ctx.next_id("issuelinks", 10000);
    ctx.put(
        "issuelinks",
        &id,
        json!({"id": id, "type": s(&t, "id"), "inward": s(&i, "key"), "outward": s(&o, "key")}),
    );
    if let Some(c) = body["comment"]["body"].as_str() {
        let cid = ctx.next_id("comments", 10000);
        let doc = json!({"id": cid, "issue": s(&o, "key"), "author": ctx.me_name(), "body": c, "created": ctx.stamp(), "updated": ctx.stamp()});
        ctx.put("comments", &cid, doc);
    }
    Ok(id)
}

fn create_link(ctx: &mut Ctx, req: &Req) -> Out {
    match insert_link(ctx, &req.body) {
        Ok(_) => Out::Empty(axum::http::StatusCode::CREATED),
        Err(e) => e,
    }
}

// ───────────────────────────── create metadata ─────────────────────────────

fn meta_fields(ctx: &Ctx, project: &str, t: &Value, creating: bool) -> Map<String, Value> {
    let mut out = Map::new();
    let tname = s(t, "name");
    let mut add = |id: &str,
                   name: &str,
                   required: bool,
                   schema: Value,
                   allowed: Option<Vec<Value>>| {
        let mut f = json!({"required": required, "schema": schema, "name": name, "fieldId": id, "hasDefaultValue": id == "priority", "operations": ["set"]});
        if let Some(a) = allowed {
            f["allowedValues"] = json!(a);
        }
        out.insert(id.to_owned(), f);
    };
    add(
        "summary",
        "Summary",
        true,
        json!({"type": "string", "system": "summary"}),
        None,
    );
    add(
        "issuetype",
        "Issue Type",
        true,
        json!({"type": "issuetype", "system": "issuetype"}),
        Some(vec![issuetype_obj(ctx, tname)]),
    );
    if creating {
        add(
            "project",
            "Project",
            true,
            json!({"type": "project", "system": "project"}),
            Some(vec![project_brief(ctx, project)]),
        );
    }
    add(
        "description",
        "Description",
        false,
        json!({"type": "string", "system": "description"}),
        None,
    );
    add(
        "priority",
        "Priority",
        false,
        json!({"type": "priority", "system": "priority"}),
        Some(
            ctx.all("priorities")
                .iter()
                .map(|p| priority_obj(ctx, s(p, "name")))
                .collect(),
        ),
    );
    add(
        "labels",
        "Labels",
        false,
        json!({"type": "array", "items": "string", "system": "labels"}),
        None,
    );
    add(
        "assignee",
        "Assignee",
        false,
        json!({"type": "user", "system": "assignee"}),
        None,
    );
    add(
        "reporter",
        "Reporter",
        true,
        json!({"type": "user", "system": "reporter"}),
        None,
    );
    add(
        "components",
        "Component/s",
        false,
        json!({"type": "array", "items": "component", "system": "components"}),
        Some(components(ctx, project)),
    );
    add(
        "fixVersions",
        "Fix Version/s",
        false,
        json!({"type": "array", "items": "version", "system": "fixVersions"}),
        Some(
            versions(ctx, project)
                .iter()
                .map(|v| version_obj(ctx, v))
                .collect(),
        ),
    );
    add(
        "duedate",
        "Due Date",
        false,
        json!({"type": "date", "system": "duedate"}),
        None,
    );
    add(
        "timetracking",
        "Time Tracking",
        false,
        json!({"type": "timetracking", "system": "timetracking"}),
        None,
    );
    if t["subtask"].as_bool().unwrap_or(false) {
        add(
            "parent",
            "Parent",
            true,
            json!({"type": "issuelink", "system": "parent"}),
            None,
        );
    }
    if tname == "Epic" {
        add(
            EPIC_NAME,
            "Epic Name",
            true,
            json!({"type": "string", "custom": "com.pyxis.greenhopper.jira:gh-epic-label", "customId": 10102}),
            None,
        );
    } else if !t["subtask"].as_bool().unwrap_or(false) {
        add(
            EPIC_LINK,
            "Epic Link",
            false,
            json!({"type": "any", "custom": "com.pyxis.greenhopper.jira:gh-epic-link", "customId": 10100}),
            None,
        );
    }
    add(
        SPRINT,
        "Sprint",
        false,
        json!({"type": "array", "items": "string", "custom": "com.pyxis.greenhopper.jira:gh-sprint", "customId": 10104}),
        None,
    );
    add(
        STORY_POINTS,
        "Story Points",
        false,
        json!({"type": "number", "custom": "com.atlassian.jira.plugin.system.customfieldtypes:float", "customId": 10106}),
        None,
    );
    for f in ctx.all("fields") {
        let applies = f["issuetypes"].as_array().is_none_or(|a| {
            a.iter()
                .any(|x| x.as_str().is_some_and(|x| x.eq_ignore_ascii_case(tname)))
        });
        if applies {
            let mut m = json!({"required": f["required"].as_bool().unwrap_or(false), "schema": f["schema"], "name": s(&f, "name"), "fieldId": s(&f, "id"), "hasDefaultValue": false, "operations": ["set"]});
            if let Some(a) = f.get("allowedValues") {
                m["allowedValues"] = a.clone();
            }
            out.insert(s(&f, "id").to_owned(), m);
        }
    }
    out
}

fn createmeta_types(ctx: &Ctx, project: &str) -> Out {
    let Some(p) = project_doc(ctx, project) else {
        return error(
            404,
            &format!("No project could be found with key '{project}'."),
        );
    };
    let types: Vec<Value> = project_types(ctx, &p)
        .iter()
        .map(|t| issuetype_obj(ctx, s(t, "name")))
        .collect();
    ok(
        json!({"maxResults": 50, "startAt": 0, "total": types.len(), "isLast": true, "values": types}),
    )
}

fn createmeta_fields(ctx: &Ctx, project: &str, type_id: &str) -> Out {
    let Some(p) = project_doc(ctx, project) else {
        return error(
            404,
            &format!("No project could be found with key '{project}'."),
        );
    };
    let Some(t) = project_types(ctx, &p)
        .into_iter()
        .find(|t| s(t, "id") == type_id)
    else {
        return error(
            404,
            &format!(
                "Issue type with id '{type_id}' does not exist or is not valid for project '{}'.",
                s(&p, "key")
            ),
        );
    };
    let fields: Vec<Value> = meta_fields(ctx, s(&p, "key"), &t, true)
        .into_values()
        .collect();
    ok(
        json!({"maxResults": 50, "startAt": 0, "total": fields.len(), "isLast": true, "values": fields}),
    )
}

fn createmeta_old(ctx: &Ctx, req: &Req) -> Out {
    let keys = req.q_list("projectKeys");
    let ids = req.q_list("projectIds");
    let type_names = req.q_list("issuetypeNames");
    let with_fields = req.q("expand").is_some_and(|e| e.contains("fields"));
    let projects: Vec<Value> = ctx
        .all("projects")
        .into_iter()
        .filter(|p| {
            (keys.is_empty() && ids.is_empty())
                || keys.iter().any(|k| k.eq_ignore_ascii_case(s(p, "key")))
                || ids.iter().any(|i| i == s(p, "id"))
        })
        .map(|p| {
            let mut brief = project_brief(ctx, s(&p, "key"));
            let types: Vec<Value> = project_types(ctx, &p)
                .iter()
                .filter(|t| {
                    type_names.is_empty()
                        || type_names
                            .iter()
                            .any(|n| n.eq_ignore_ascii_case(s(t, "name")))
                })
                .map(|t| {
                    let mut o = issuetype_obj(ctx, s(t, "name"));
                    if with_fields {
                        o["fields"] = Value::Object(meta_fields(ctx, s(&p, "key"), t, true));
                    }
                    o
                })
                .collect();
            brief["issuetypes"] = json!(types);
            brief
        })
        .collect();
    ok(json!({"expand": "projects", "projects": projects}))
}

// ───────────────────────────── projects ─────────────────────────────

fn project_route(ctx: &mut Ctx, req: &Req, m: &str, key: &str, tail: &[&str]) -> Out {
    let Some(p) = project_doc(ctx, key) else {
        return error(404, &format!("No project could be found with key '{key}'."));
    };
    let pk = s(&p, "key").to_owned();
    match (m, tail) {
        ("GET", []) => ok(project_obj(ctx, &p, true)),
        ("GET", ["versions"]) => ok(json!(
            versions(ctx, &pk)
                .iter()
                .map(|v| version_obj(ctx, v))
                .collect::<Vec<_>>()
        )),
        ("GET", ["version"]) => {
            let all: Vec<Value> = versions(ctx, &pk)
                .iter()
                .map(|v| version_obj(ctx, v))
                .collect();
            ok(
                json!({"self": ctx.api(&format!("project/{pk}/version")), "maxResults": 50, "startAt": 0, "total": all.len(), "isLast": true, "values": all}),
            )
        }
        ("GET", ["components"]) => ok(json!(components(ctx, &pk))),
        ("GET", ["statuses"]) => {
            let statuses: Vec<Value> = ctx
                .all("statuses")
                .iter()
                .map(|x| status_obj(ctx, s(x, "name")))
                .collect();
            ok(json!(project_types(ctx, &p).iter().map(|t| json!({"self": ctx.api(&format!("issuetype/{}", s(t, "id"))), "id": s(t, "id"), "name": s(t, "name"), "subtask": t["subtask"].as_bool().unwrap_or(false), "statuses": statuses})).collect::<Vec<_>>()))
        }
        ("GET", ["role"]) => {
            let roles: Map<String, Value> = role_names(&p)
                .into_iter()
                .enumerate()
                .map(|(i, r)| {
                    (
                        r,
                        json!(ctx.api(&format!("project/{}/role/{}", s(&p, "id"), 10002 + i))),
                    )
                })
                .collect();
            ok(Value::Object(roles))
        }
        ("GET", ["role", rid]) => {
            let names = role_names(&p);
            let idx = rid
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(10002))
                .filter(|i| *i < names.len());
            let Some(i) = idx else {
                return error(
                    404,
                    &format!(
                        "Can not retrieve a role actor for a null project role. Role id: {rid}"
                    ),
                );
            };
            let name = &names[i];
            let members: Vec<String> = p["roles"][name]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let actors: Vec<Value> = members.iter().enumerate().filter_map(|(j, n)| ctx.user_doc(n).map(|u| json!({"id": 10100 + j, "displayName": s(&u, "displayName"), "type": "atlassian-user-role-actor", "name": s(&u, "name"), "avatarUrl": format!("{}/secure/useravatar?size=xsmall&ownerId={}", ctx.base, s(&u, "name"))}))).collect();
            ok(
                json!({"self": ctx.api(&format!("project/{}/role/{rid}", s(&p, "id"))), "name": name, "id": rid.parse::<i64>().unwrap_or(0), "description": format!("{name} of the project"), "actors": actors}),
            )
        }
        ("GET", ["permissionscheme"]) => ok(
            json!({"expand": "permissions,user,group,projectRole,field,all", "id": 10000, "self": ctx.api("permissionscheme/10000"), "name": "Default Permission Scheme", "description": "This is the default Permission Scheme. Any new projects that are created will be assigned this scheme."}),
        ),
        ("GET", ["notificationscheme"]) => ok(
            json!({"expand": "notificationSchemeEvents,user,group,projectRole,field,all", "id": 10000, "self": ctx.api("notificationscheme/10000"), "name": "Default Notification Scheme", "description": ""}),
        ),
        ("GET", ["properties", ..]) => ok(json!({"keys": []})),
        _ => {
            let _ = req;
            not_found_page()
        }
    }
}

fn create_version(ctx: &mut Ctx, body: &Value) -> Out {
    let project = body["project"].as_str().map(str::to_owned).or_else(|| {
        id_num(&body["projectId"]).and_then(|id| {
            ctx.all("projects")
                .into_iter()
                .find(|p| id_num(&p["id"]) == Some(id))
                .map(|p| s(&p, "key").to_owned())
        })
    });
    let Some(pk) = project
        .and_then(|p| project_doc(ctx, &p))
        .map(|p| s(&p, "key").to_owned())
    else {
        return field_errors(json!({"project": "Project must be specified to create a version."}));
    };
    let name = s(body, "name").trim().to_owned();
    if name.is_empty() {
        return field_errors(json!({"name": "You must specify a valid version name"}));
    }
    if versions(ctx, &pk)
        .iter()
        .any(|v| s(v, "name").eq_ignore_ascii_case(&name))
    {
        return field_errors(
            json!({"name": "A version with this name already exists in this project."}),
        );
    }
    for k in ["releaseDate", "startDate"] {
        if let Some(d) = body[k].as_str()
            && parse_time(d).is_none()
        {
            return field_errors(
                json!({k: format!("Invalid {} date format. Use yyyy-MM-dd.", if k == "releaseDate" { "release" } else { "start" })}),
            );
        }
    }
    let id = ctx.next_id("versions", 10000);
    let mut v = json!({"id": id, "project": pk, "name": name, "archived": body["archived"].as_bool().unwrap_or(false), "released": body["released"].as_bool().unwrap_or(false)});
    for k in ["description", "releaseDate", "startDate"] {
        if let Some(x) = body[k].as_str() {
            v[k] = json!(x);
        }
    }
    ctx.put("versions", &id, v.clone());
    created(version_obj(ctx, &v))
}

fn update_version(ctx: &mut Ctx, id: &str, body: &Value) -> Out {
    let Some(mut v) = ctx.get("versions", id) else {
        return error(404, &format!("Could not find version for id '{id}'"));
    };
    for k in ["name", "description", "releaseDate", "startDate"] {
        if let Some(x) = body.get(k) {
            v[k] = x.clone();
        }
    }
    for k in ["released", "archived"] {
        if let Some(x) = body[k].as_bool() {
            v[k] = json!(x);
        }
    }
    ctx.put("versions", id, v.clone());
    ok(version_obj(ctx, &v))
}
