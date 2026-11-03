//! Projects (+ updates, members, issues), milestones, initiatives (+ updates), roadmaps and
//! documents.

use serde_json::{Value, json};

use crate::{
    issues::{completed, order, summaries, visible_issues},
    store::{
        Ctx, R, arg, arg_bool, arg_i64, arg_strs, get_or_null, not_found, obj, opt, s, sdk, slugify,
    },
};

const UUID_LEN: usize = 36;

fn is_uuid(text: &str) -> bool {
    text.len() == UUID_LEN
        && text.chars().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn limit(args: &Value, default: i64) -> usize {
    arg_i64(args, "limit").unwrap_or(default).max(0) as usize
}

fn archived_ok(args: &Value, v: &Value) -> bool {
    arg_bool(args, "includeArchived").unwrap_or(false) || opt(v, "archivedAt").is_none()
}

// ───────────────────────────── projects ─────────────────────────────

fn statuses(ctx: &Ctx) -> Vec<Value> {
    let list = ctx.all("project_statuses");
    if !list.is_empty() {
        return list;
    }
    [
        (
            "c0a1b2c3-0001-4a5b-8c6d-7e8f90a1b2c3",
            "Backlog",
            "backlog",
            0,
        ),
        (
            "c0a1b2c3-0002-4a5b-8c6d-7e8f90a1b2c3",
            "Planned",
            "planned",
            1,
        ),
        (
            "c0a1b2c3-0003-4a5b-8c6d-7e8f90a1b2c3",
            "In Progress",
            "started",
            2,
        ),
        (
            "c0a1b2c3-0004-4a5b-8c6d-7e8f90a1b2c3",
            "Completed",
            "completed",
            3,
        ),
        (
            "c0a1b2c3-0005-4a5b-8c6d-7e8f90a1b2c3",
            "Canceled",
            "canceled",
            4,
        ),
    ]
    .iter()
    .map(|(id, n, t, p)| json!({"id": id, "name": n, "type": t, "position": p}))
    .collect()
}

fn norm_status(v: &str) -> String {
    v.trim()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .collect()
}

/// `resolveProjectStatusId`: a UUID passes through; names/types are matched.
fn resolve_status(ctx: &Ctx, state: Option<&str>) -> Result<Option<Value>, String> {
    let Some(v) = state else {
        return Ok(None);
    };
    let all = statuses(ctx);
    if is_uuid(v) {
        return all
            .into_iter()
            .find(|st| s(st, "id") == v)
            .map(Some)
            .ok_or_else(|| not_found("ProjectStatus"));
    }
    let key = norm_status(v);
    let mut matches: Vec<Value> = all
        .iter()
        .filter(|st| norm_status(s(st, "name")) == key || norm_status(s(st, "type")) == key)
        .cloned()
        .collect();
    matches.sort_by_key(|st| st["position"].as_i64().unwrap_or(i64::MAX));
    match matches.into_iter().next() {
        Some(st) => Ok(Some(st)),
        None => {
            let names: Vec<&str> = all.iter().map(|st| s(st, "name")).collect();
            Err(format!(
                "Project status \"{v}\" not found. Use a project status ID or one of: {}",
                names.join(", ")
            ))
        }
    }
}

pub fn project_out(ctx: &Ctx, p: &Value) -> Value {
    let mut o = sdk(vec![
        ("id", json!(s(p, "id"))),
        ("name", json!(s(p, "name"))),
        ("description", json!(s(p, "description"))),
        ("content", get_or_null(p, "content")),
        (
            "state",
            json!(match s(p, "state") {
                "" => "backlog",
                x => x,
            }),
        ),
    ]);
    let teams: Vec<Value> = p["teamIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| ctx.get("teams", t.as_str().unwrap_or("")))
                .map(|t| json!({"id": s(&t, "id"), "name": s(&t, "name")}))
                .collect()
        })
        .unwrap_or_default();
    let m = o.as_object_mut().expect("object");
    for f in [
        "trashed",
        "slackChannelId",
        "microsoftTeamsChannelId",
        "startDate",
        "targetDate",
        "icon",
        "color",
    ] {
        m.insert(f.into(), get_or_null(p, f));
    }
    m.insert("url".into(), json!(ctx.project_url(p)));
    m.insert("lead".into(), ctx.user_ref(s(p, "leadId")));
    m.insert("teams".into(), Value::Array(teams));
    o
}

pub fn get_projects(ctx: &mut Ctx, _args: &Value) -> R {
    Ok(Value::Array(
        ctx.all("projects")
            .iter()
            .filter(|p| opt(p, "archivedAt").is_none() && !p["trashed"].as_bool().unwrap_or(false))
            .map(|p| project_out(ctx, p))
            .collect(),
    ))
}

pub fn get_project(ctx: &mut Ctx, args: &Value) -> R {
    let p = ctx.need("projects", arg(args, "id").unwrap_or(""), "Project")?;
    Ok(project_out(ctx, &p))
}

fn ids_arg(args: &Value, k: &str) -> Option<Vec<String>> {
    match args.get(k) {
        Some(Value::String(x)) => Some(vec![x.clone()]),
        Some(Value::Array(_)) => arg_strs(args, k),
        _ => None,
    }
}

fn apply_project_fields(ctx: &Ctx, args: &Value, p: &mut Value) -> Result<(), String> {
    for f in ["name", "description", "content", "icon", "color"] {
        if let Some(v) = args.get(f).and_then(Value::as_str) {
            p[f] = json!(v);
        }
    }
    for f in ["startDate", "targetDate"] {
        if let Some(v) = arg(args, f) {
            p[f] = json!(v.get(0..10).unwrap_or(v));
        }
    }
    if let Some(so) = args.get("sortOrder").filter(|x| x.is_number()) {
        p["sortOrder"] = so.clone();
    }
    if let Some(l) = arg(args, "leadId") {
        ctx.need("users", l, "User")?;
        p["leadId"] = json!(l);
    }
    if let Some(m) = ids_arg(args, "memberIds") {
        for u in &m {
            ctx.need("users", u, "User")?;
        }
        p["memberIds"] = json!(m);
    }
    if let Some(st) = resolve_status(ctx, arg(args, "state"))? {
        p["statusId"] = json!(s(&st, "id"));
        p["state"] = json!(s(&st, "type"));
    }
    Ok(())
}

pub fn create_project(ctx: &mut Ctx, args: &Value) -> R {
    let teams = ids_arg(args, "teamIds").unwrap_or_default();
    if teams.is_empty() {
        return Err("Argument Validation Error - teamIds should not be empty.".into());
    }
    for t in &teams {
        ctx.need("teams", t, "Team")?;
    }
    let id = ctx.uuid("project");
    let slug = ctx.slug_id("project_slug");
    let now = ctx.iso_now();
    let backlog = statuses(ctx)
        .into_iter()
        .find(|st| s(st, "type") == "backlog")
        .unwrap_or(Value::Null);
    let mut p = json!({
        "id": id, "slugId": slug, "name": s(args, "name"), "description": "", "teamIds": teams,
        "memberIds": [], "state": "backlog", "statusId": s(&backlog, "id"), "createdAt": now, "updatedAt": now,
        "creatorId": ctx.viewer_id(), "progress": 0, "trashed": null,
    });
    apply_project_fields(ctx, args, &mut p)?;
    ctx.put("projects", &id, p.clone());
    Ok(project_out(ctx, &p))
}

pub fn update_project(ctx: &mut Ctx, args: &Value) -> R {
    let mut p = ctx.need("projects", arg(args, "id").unwrap_or(""), "Project")?;
    apply_project_fields(ctx, args, &mut p)?;
    p["updatedAt"] = json!(ctx.iso_now());
    ctx.save("projects", p.clone());
    Ok(project_out(ctx, &p))
}

pub fn archive_project(ctx: &mut Ctx, args: &Value) -> R {
    let mut p = ctx.need("projects", arg(args, "projectId").unwrap_or(""), "Project")?;
    p["archivedAt"] = json!(ctx.iso_now());
    ctx.save("projects", p.clone());
    Ok(
        json!({"success": true, "project": {"id": s(&p, "id"), "name": s(&p, "name"), "state": s(&p, "state"), "archivedAt": s(&p, "archivedAt")}}),
    )
}

pub fn project_members(ctx: &mut Ctx, args: &Value) -> R {
    let p = ctx.need("projects", arg(args, "projectId").unwrap_or(""), "Project")?;
    let include_disabled = arg_bool(args, "includeDisabled").unwrap_or(false);
    let members: Vec<Value> = p["memberIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|u| ctx.get("users", u.as_str().unwrap_or("")))
                .filter(|u| {
                    include_disabled || u.get("active").and_then(Value::as_bool).unwrap_or(true)
                })
                .take(limit(args, 25))
                .map(|u| ctx.simple_user(s(&u, "id")))
                .collect()
        })
        .unwrap_or_default();
    Ok(Value::Array(members))
}

pub fn change_member(ctx: &mut Ctx, args: &Value, add: bool) -> R {
    let project_id = arg(args, "projectId").unwrap_or("").to_owned();
    let user_id = arg(args, "userId").unwrap_or("").to_owned();
    let mut p = ctx.need("projects", &project_id, "Project")?;
    ctx.need("users", &user_id, "User")?;
    let mut members: Vec<String> = p["memberIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if add {
        if !members.contains(&user_id) {
            members.push(user_id.clone());
        }
    } else {
        members.retain(|m| *m != user_id);
    }
    p["memberIds"] = json!(members);
    p["updatedAt"] = json!(ctx.iso_now());
    ctx.save("projects", p);
    Ok(json!({"success": true, "projectId": project_id, "userId": user_id}))
}

pub fn project_issues(ctx: &mut Ctx, args: &Value) -> R {
    let pid = arg(args, "projectId").unwrap_or("").to_owned();
    Ok(Value::Array(summaries(ctx, args, |i| {
        s(i, "projectId") == pid
    })))
}

pub fn add_issue_to_project(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let p = ctx.need("projects", arg(args, "projectId").unwrap_or(""), "Project")?;
    let issue_id = s(&issue, "id").to_owned();
    ctx.history(
        &issue_id,
        "project",
        get_or_null(&issue, "projectId"),
        json!(s(&p, "id")),
    );
    issue["projectId"] = json!(s(&p, "id"));
    issue["updatedAt"] = json!(ctx.iso_now());
    ctx.put("issues", &key, issue.clone());
    Ok(json!({"success": true, "issue": {
        "id": s(&issue, "id"), "identifier": s(&issue, "identifier"), "title": s(&issue, "title"),
        "project": {"id": s(&p, "id"), "name": s(&p, "name")},
    }}))
}

pub fn remove_issue_from_project(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let pid = arg(args, "projectId").unwrap_or("").to_owned();
    if s(&issue, "projectId") != pid {
        return Err(format!(
            "Issue {} is not associated with project {pid}",
            s(&issue, "identifier")
        ));
    }
    let p = ctx.need("projects", &pid, "Project")?;
    issue["projectId"] = Value::Null;
    issue["projectMilestoneId"] = Value::Null;
    issue["updatedAt"] = json!(ctx.iso_now());
    ctx.put("issues", &key, issue.clone());
    Ok(json!({
        "success": true,
        "issue": {"id": s(&issue, "id"), "identifier": s(&issue, "identifier"), "title": s(&issue, "title")},
        "project": {"id": s(&p, "id"), "name": s(&p, "name")},
    }))
}

// ───────────────────────────── project / initiative updates ─────────────────────────────

fn update_out(
    ctx: &Ctx,
    u: &Value,
    parent_coll: &str,
    parent_field: &str,
    initiative: bool,
) -> Value {
    let mut fields = vec![
        ("id", json!(s(u, "id"))),
        ("body", json!(s(u, "body"))),
        (
            "health",
            json!(match s(u, "health") {
                "" => "onTrack",
                h => h,
            }),
        ),
        ("diff", get_or_null(u, "diff")),
        ("diffMarkdown", get_or_null(u, "diffMarkdown")),
        (
            "isDiffHidden",
            json!(u["isDiffHidden"].as_bool().unwrap_or(false)),
        ),
    ];
    if initiative {
        fields.push(("isStale", json!(u["isStale"].as_bool().unwrap_or(false))));
        fields.push((
            "commentCount",
            json!(
                ctx.all("comments")
                    .iter()
                    .filter(|c| s(c, "initiativeUpdateId") == s(u, "id"))
                    .count()
            ),
        ));
    }
    let parent = ctx
        .get(parent_coll, s(u, parent_field))
        .unwrap_or(Value::Null);
    let url = if initiative {
        format!(
            "https://linear.app/{}/initiative/{}/updates#initiative-update-{}",
            ctx.url_key(),
            slugify(s(&parent, "name")),
            s(u, "slugId")
        )
    } else {
        format!(
            "{}/updates#project-update-{}",
            ctx.project_url(&parent),
            s(u, "slugId")
        )
    };
    fields.extend([
        ("url", json!(url)),
        ("slugId", get_or_null(u, "slugId")),
        ("archivedAt", get_or_null(u, "archivedAt")),
        ("editedAt", get_or_null(u, "editedAt")),
        ("createdAt", json!(s(u, "createdAt"))),
        ("updatedAt", json!(s(u, "updatedAt"))),
        ("user", ctx.user_ref(s(u, "userId"))),
        (
            if initiative { "initiative" } else { "project" },
            ctx.named_ref(parent_coll, s(u, parent_field)),
        ),
    ]);
    obj(fields)
}

fn check_health(args: &Value) -> Result<(), String> {
    match arg(args, "health") {
        Some(h) if !["onTrack", "atRisk", "offTrack"].contains(&h) => Err(format!(
            "Argument Validation Error - health must be one of: onTrack, atRisk, offTrack (got \"{h}\")."
        )),
        _ => Ok(()),
    }
}

pub fn create_update(ctx: &mut Ctx, args: &Value, initiative: bool) -> R {
    let (coll, field, kind, ucoll) = if initiative {
        (
            "initiatives",
            "initiativeId",
            "Initiative",
            "initiative_updates",
        )
    } else {
        ("projects", "projectId", "Project", "project_updates")
    };
    let parent = ctx.need(coll, arg(args, field).unwrap_or(""), kind)?;
    check_health(args)?;
    let id = ctx.uuid(ucoll);
    let slug = ctx.slug_id("update_slug");
    let now = ctx.iso_now();
    let doc = json!({
        "id": id, field: s(&parent, "id"), "body": s(args, "body"),
        "health": arg(args, "health").unwrap_or("onTrack"),
        "isDiffHidden": arg_bool(args, "isDiffHidden").unwrap_or(false),
        "slugId": slug, "userId": ctx.viewer_id(), "createdAt": now, "updatedAt": now,
    });
    ctx.put(ucoll, &id, doc.clone());
    Ok(update_out(ctx, &doc, coll, field, initiative))
}

pub fn edit_update(ctx: &mut Ctx, args: &Value, initiative: bool) -> R {
    let (coll, field, kind, ucoll) = if initiative {
        (
            "initiatives",
            "initiativeId",
            "InitiativeUpdate",
            "initiative_updates",
        )
    } else {
        ("projects", "projectId", "ProjectUpdate", "project_updates")
    };
    let mut u = ctx.need(ucoll, arg(args, "id").unwrap_or(""), kind)?;
    check_health(args)?;
    if let Some(b) = args.get("body").and_then(Value::as_str) {
        u["body"] = json!(b);
        u["editedAt"] = json!(ctx.iso_now());
    }
    if let Some(h) = arg(args, "health") {
        u["health"] = json!(h);
    }
    if let Some(d) = arg_bool(args, "isDiffHidden") {
        u["isDiffHidden"] = json!(d);
    }
    u["updatedAt"] = json!(ctx.iso_now());
    ctx.save(ucoll, u.clone());
    Ok(update_out(ctx, &u, coll, field, initiative))
}

pub fn get_update(ctx: &mut Ctx, args: &Value, initiative: bool) -> R {
    let (coll, field, kind, ucoll) = if initiative {
        (
            "initiatives",
            "initiativeId",
            "InitiativeUpdate",
            "initiative_updates",
        )
    } else {
        ("projects", "projectId", "ProjectUpdate", "project_updates")
    };
    let u = ctx.need(ucoll, arg(args, "id").unwrap_or(""), kind)?;
    Ok(update_out(ctx, &u, coll, field, initiative))
}

pub fn list_updates(ctx: &mut Ctx, args: &Value, initiative: bool) -> R {
    let (coll, field, kind, ucoll) = if initiative {
        (
            "initiatives",
            "initiativeId",
            "Initiative",
            "initiative_updates",
        )
    } else {
        ("projects", "projectId", "Project", "project_updates")
    };
    let parent = ctx.need(coll, arg(args, field).unwrap_or(""), kind)?;
    let mut list: Vec<Value> = ctx
        .all(ucoll)
        .into_iter()
        .filter(|u| s(u, field) == s(&parent, "id") && opt(u, "archivedAt").is_none())
        .collect();
    list.sort_by(|a, b| s(b, "createdAt").cmp(s(a, "createdAt")));
    Ok(Value::Array(
        list.iter()
            .take(limit(args, 25))
            .map(|u| update_out(ctx, u, coll, field, initiative))
            .collect(),
    ))
}

pub fn archive_update(ctx: &mut Ctx, args: &Value, initiative: bool, archive: bool) -> R {
    let (kind, ucoll) = if initiative {
        ("InitiativeUpdate", "initiative_updates")
    } else {
        ("ProjectUpdate", "project_updates")
    };
    let id = arg(args, "id").unwrap_or("").to_owned();
    let mut u = ctx.need(ucoll, &id, kind)?;
    u["archivedAt"] = if archive {
        json!(ctx.iso_now())
    } else {
        Value::Null
    };
    ctx.save(ucoll, u);
    Ok(json!({"success": true, "id": id}))
}

pub fn delete_project_update(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    ctx.need("project_updates", &id, "ProjectUpdate")?;
    ctx.remove("project_updates", &id);
    Ok(json!({"success": true, "id": id}))
}

// ───────────────────────────── milestones ─────────────────────────────

fn milestone_status(ctx: &Ctx, m: &Value) -> String {
    if let Some(st) = m.get("status").and_then(Value::as_str) {
        return st.to_owned();
    }
    let issues: Vec<Value> = visible_issues(ctx)
        .into_iter()
        .filter(|i| s(i, "projectMilestoneId") == s(m, "id"))
        .collect();
    if !issues.is_empty() && issues.iter().all(|i| completed(ctx, i)) {
        return "done".into();
    }
    match crate::store::parse_time(s(m, "targetDate")) {
        Some(t) if t + 86_400 <= ctx.now => "overdue".into(),
        _ => "unstarted".into(),
    }
}

fn milestone_out(ctx: &Ctx, m: &Value) -> Value {
    let issues: Vec<Value> = visible_issues(ctx)
        .into_iter()
        .filter(|i| s(i, "projectMilestoneId") == s(m, "id"))
        .collect();
    let progress = if issues.is_empty() {
        0.0
    } else {
        issues.iter().filter(|i| completed(ctx, i)).count() as f64 / issues.len() as f64
    };
    let mut o = sdk(vec![
        ("id", json!(s(m, "id"))),
        ("name", json!(s(m, "name"))),
        ("description", get_or_null(m, "description")),
        ("status", json!(milestone_status(ctx, m))),
        ("progress", json!(progress)),
        ("sortOrder", m.get("sortOrder").cloned().unwrap_or(json!(0))),
        ("targetDate", get_or_null(m, "targetDate")),
        ("createdAt", json!(s(m, "createdAt"))),
        ("updatedAt", json!(s(m, "updatedAt"))),
        ("archivedAt", get_or_null(m, "archivedAt")),
    ]);
    o["project"] = match ctx.get("projects", s(m, "projectId")) {
        Some(p) => json!({"id": s(&p, "id"), "name": s(&p, "name")}),
        None => Value::Null,
    };
    o
}

pub fn get_milestones(ctx: &mut Ctx, args: &Value) -> R {
    if let Some(pid) = arg(args, "projectId") {
        let p = ctx.need("projects", pid, "Project")?;
        if let Some(t) = arg(args, "teamId")
            && !p["teamIds"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == t))
        {
            return Ok(json!([]));
        }
    }
    let list: Vec<Value> = order(args, ctx.all("milestones"))
        .into_iter()
        .filter(|m| archived_ok(args, m))
        .take(limit(args, 50))
        .map(|m| milestone_out(ctx, &m))
        .filter(|m| arg(args, "projectId").is_none_or(|p| m["project"]["id"] == p))
        .filter(|m| arg(args, "status").is_none_or(|st| m["status"] == st))
        .collect();
    Ok(Value::Array(list))
}

pub fn get_milestone(ctx: &mut Ctx, args: &Value) -> R {
    let m = ctx.need(
        "milestones",
        arg(args, "id").unwrap_or(""),
        "ProjectMilestone",
    )?;
    Ok(milestone_out(ctx, &m))
}

pub fn create_milestone(ctx: &mut Ctx, args: &Value) -> R {
    let p = ctx.need("projects", arg(args, "projectId").unwrap_or(""), "Project")?;
    let id = ctx.uuid("milestone");
    let now = ctx.iso_now();
    let sort = ctx
        .all("milestones")
        .iter()
        .filter(|m| s(m, "projectId") == s(&p, "id"))
        .count() as f64
        * 1000.0;
    let doc = json!({
        "id": id, "projectId": s(&p, "id"), "name": s(args, "name"), "description": arg(args, "description"),
        "targetDate": arg(args, "targetDate"), "sortOrder": args.get("sortOrder").cloned().unwrap_or(json!(sort)),
        "createdAt": now, "updatedAt": now,
    });
    ctx.put("milestones", &id, doc.clone());
    Ok(milestone_out(ctx, &doc))
}

pub fn update_milestone(ctx: &mut Ctx, args: &Value) -> R {
    let mut m = ctx.need(
        "milestones",
        arg(args, "id").unwrap_or(""),
        "ProjectMilestone",
    )?;
    let mut any = false;
    for f in ["name", "description", "targetDate"] {
        if let Some(v) = arg(args, f) {
            m[f] = json!(v);
            any = true;
        }
    }
    if let Some(p) = arg(args, "projectId") {
        ctx.need("projects", p, "Project")?;
        m["projectId"] = json!(p);
        any = true;
    }
    if let Some(so) = args.get("sortOrder").filter(|x| x.is_number()) {
        m["sortOrder"] = so.clone();
        any = true;
    }
    if !any {
        return Err("At least one milestone field must be provided".into());
    }
    m["updatedAt"] = json!(ctx.iso_now());
    ctx.save("milestones", m.clone());
    Ok(milestone_out(ctx, &m))
}

pub fn archive_milestone(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let m = ctx.need("milestones", id, "ProjectMilestone")?;
    ctx.remove("milestones", s(&m, "id"));
    Ok(json!({"success": true, "milestone": {"id": s(&m, "id"), "name": s(&m, "name")}}))
}

// ───────────────────────────── initiatives ─────────────────────────────

fn initiative_url(ctx: &Ctx, i: &Value) -> String {
    format!(
        "https://linear.app/{}/initiative/{}-{}",
        ctx.url_key(),
        slugify(s(i, "name")),
        s(i, "slugId")
    )
}

fn initiative_projects(ctx: &Ctx, initiative_id: &str, include_archived: bool) -> Vec<Value> {
    ctx.all("initiative_projects")
        .into_iter()
        .filter(|r| s(r, "initiativeId") == initiative_id)
        .filter_map(|r| ctx.get("projects", s(&r, "projectId")))
        .filter(|p| include_archived || opt(p, "archivedAt").is_none())
        .collect()
}

fn initiative_out(ctx: &Ctx, i: &Value, projects: bool) -> Value {
    let mut o = sdk(vec![
        ("id", json!(s(i, "id"))),
        ("name", json!(s(i, "name"))),
        ("description", get_or_null(i, "description")),
        ("content", get_or_null(i, "content")),
        ("icon", get_or_null(i, "icon")),
        ("color", get_or_null(i, "color")),
        (
            "status",
            json!(match s(i, "status") {
                "" => "Planned",
                x => x,
            }),
        ),
        ("targetDate", get_or_null(i, "targetDate")),
        ("sortOrder", i.get("sortOrder").cloned().unwrap_or(json!(0))),
    ]);
    let m = o.as_object_mut().expect("object");
    m.insert("owner".into(), ctx.user_ref_email(s(i, "ownerId")));
    if projects {
        let list: Vec<Value> = initiative_projects(ctx, s(i, "id"), false)
            .iter()
            .map(|p| json!({"id": s(p, "id"), "name": s(p, "name"), "state": s(p, "state")}))
            .collect();
        m.insert("projects".into(), Value::Array(list));
    }
    m.insert("url".into(), json!(initiative_url(ctx, i)));
    o
}

pub fn get_initiatives(ctx: &mut Ctx, args: &Value) -> R {
    let include_archived = arg_bool(args, "includeArchived").unwrap_or(false);
    Ok(Value::Array(
        ctx.all("initiatives")
            .iter()
            .filter(|i| include_archived || opt(i, "archivedAt").is_none())
            .filter(|i| !i["trashed"].as_bool().unwrap_or(false))
            .take(match arg_i64(args, "limit") {
                Some(0) | None => 50,
                Some(n) => n as usize,
            })
            .map(|i| initiative_out(ctx, i, false))
            .collect(),
    ))
}

pub fn get_initiative(ctx: &mut Ctx, args: &Value) -> R {
    let i = ctx.need(
        "initiatives",
        arg(args, "initiativeId").unwrap_or(""),
        "Initiative",
    )?;
    Ok(initiative_out(
        ctx,
        &i,
        arg_bool(args, "includeProjects").unwrap_or(true),
    ))
}

fn initiative_brief(ctx: &Ctx, i: &Value) -> Value {
    sdk(vec![
        ("id", json!(s(i, "id"))),
        ("name", json!(s(i, "name"))),
        ("description", get_or_null(i, "description")),
        ("status", json!(s(i, "status"))),
        ("url", json!(initiative_url(ctx, i))),
    ])
}

fn apply_initiative(ctx: &Ctx, args: &Value, i: &mut Value) -> Result<(), String> {
    for f in [
        "name",
        "description",
        "content",
        "targetDate",
        "icon",
        "color",
    ] {
        if let Some(v) = args.get(f).and_then(Value::as_str) {
            i[f] = json!(v);
        }
    }
    if let Some(st) = arg(args, "status") {
        if !["Planned", "Active", "Completed"].contains(&st) {
            return Err(format!(
                "Argument Validation Error - status must be one of: Planned, Active, Completed (got \"{st}\")."
            ));
        }
        i["status"] = json!(st);
    }
    if let Some(o) = arg(args, "ownerId") {
        ctx.need("users", o, "User")?;
        i["ownerId"] = json!(o);
    }
    if let Some(so) = args.get("sortOrder").filter(|x| x.is_number()) {
        i["sortOrder"] = so.clone();
    }
    Ok(())
}

pub fn create_initiative(ctx: &mut Ctx, args: &Value) -> R {
    let id = ctx.uuid("initiative");
    let slug = ctx.slug_id("initiative_slug");
    let now = ctx.iso_now();
    let mut i = json!({"id": id, "slugId": slug, "status": "Planned", "createdAt": now, "updatedAt": now, "sortOrder": 0, "creatorId": ctx.viewer_id()});
    apply_initiative(ctx, args, &mut i)?;
    ctx.put("initiatives", &id, i.clone());
    Ok(initiative_brief(ctx, &i))
}

pub fn update_initiative(ctx: &mut Ctx, args: &Value) -> R {
    let mut i = ctx.need(
        "initiatives",
        arg(args, "initiativeId").unwrap_or(""),
        "Initiative",
    )?;
    apply_initiative(ctx, args, &mut i)?;
    i["updatedAt"] = json!(ctx.iso_now());
    ctx.save("initiatives", i.clone());
    Ok(initiative_brief(ctx, &i))
}

pub fn archive_initiative(ctx: &mut Ctx, args: &Value, archive: bool) -> R {
    let mut i = ctx.need(
        "initiatives",
        arg(args, "initiativeId").unwrap_or(""),
        "Initiative",
    )?;
    i["archivedAt"] = if archive {
        json!(ctx.iso_now())
    } else {
        Value::Null
    };
    ctx.save("initiatives", i.clone());
    Ok(json!({"success": true, "entity": {"id": s(&i, "id"), "name": s(&i, "name")}}))
}

pub fn delete_initiative(ctx: &mut Ctx, args: &Value) -> R {
    let mut i = ctx.need(
        "initiatives",
        arg(args, "initiativeId").unwrap_or(""),
        "Initiative",
    )?;
    i["trashed"] = json!(true);
    ctx.save("initiatives", i);
    Ok(json!({"success": true}))
}

pub fn get_initiative_projects(ctx: &mut Ctx, args: &Value) -> R {
    let i = ctx.need(
        "initiatives",
        arg(args, "initiativeId").unwrap_or(""),
        "Initiative",
    )?;
    let list: Vec<Value> = initiative_projects(
        ctx,
        s(&i, "id"),
        arg_bool(args, "includeArchived").unwrap_or(false),
    )
    .iter()
    .take(50)
    .map(|p| {
        let mut o = sdk(vec![
            ("id", json!(s(p, "id"))),
            ("name", json!(s(p, "name"))),
            ("description", json!(s(p, "description"))),
            ("state", json!(s(p, "state"))),
            ("progress", p.get("progress").cloned().unwrap_or(json!(0))),
            ("startDate", get_or_null(p, "startDate")),
            ("targetDate", get_or_null(p, "targetDate")),
        ]);
        o["teams"] = project_out(ctx, p)["teams"].clone();
        o["url"] = json!(ctx.project_url(p));
        o
    })
    .collect();
    Ok(Value::Array(list))
}

pub fn add_project_to_initiative(ctx: &mut Ctx, args: &Value) -> R {
    let p = ctx.need("projects", arg(args, "projectId").unwrap_or(""), "Project")?;
    let i = ctx.need(
        "initiatives",
        arg(args, "initiativeId").unwrap_or(""),
        "Initiative",
    )?;
    if ctx
        .all("initiative_projects")
        .iter()
        .any(|r| s(r, "initiativeId") == s(&i, "id") && s(r, "projectId") == s(&p, "id"))
    {
        return Err("Invalid input - Project is already part of this initiative.".into());
    }
    let id = ctx.uuid("initiative_project");
    ctx.put(
        "initiative_projects",
        &id,
        json!({"id": id, "initiativeId": s(&i, "id"), "projectId": s(&p, "id"), "createdAt": ctx.iso_now()}),
    );
    Ok(
        json!({"success": true, "project": {"id": s(&p, "id"), "name": s(&p, "name"), "initiative": {"id": s(&i, "id"), "name": s(&i, "name")}}}),
    )
}

pub fn remove_project_from_initiative(ctx: &mut Ctx, args: &Value) -> R {
    let pid = arg(args, "projectId").unwrap_or("").to_owned();
    let iid = arg(args, "initiativeId").unwrap_or("").to_owned();
    let p = ctx.need("projects", &pid, "Project")?;
    let i = ctx.need("initiatives", &iid, "Initiative")?;
    let rel = ctx
        .all("initiative_projects")
        .into_iter()
        .find(|r| s(r, "initiativeId") == s(&i, "id") && s(r, "projectId") == s(&p, "id"))
        .ok_or_else(|| format!("Project {pid} is not associated with initiative {iid}"))?;
    ctx.remove("initiative_projects", s(&rel, "id"));
    Ok(
        json!({"success": true, "project": {"id": s(&p, "id"), "name": s(&p, "name")}, "initiative": {"id": s(&i, "id"), "name": s(&i, "name")}}),
    )
}

// ───────────────────────────── roadmaps ─────────────────────────────

fn roadmaps_enabled(ctx: &Ctx) -> Result<(), String> {
    let org = ctx.org();
    if org["roadmapEnabled"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        Err(format!(
            "Roadmaps are not enabled for organization {}",
            s(&org, "name")
        ))
    }
}

fn roadmap_out(ctx: &Ctx, r: &Value, projects: bool) -> Value {
    let mut o = obj(vec![
        ("id", json!(s(r, "id"))),
        ("name", json!(s(r, "name"))),
        ("description", get_or_null(r, "description")),
        ("color", get_or_null(r, "color")),
        ("slugId", json!(s(r, "slugId"))),
        ("sortOrder", r.get("sortOrder").cloned().unwrap_or(json!(0))),
        ("owner", ctx.user_ref_email(s(r, "ownerId"))),
        ("creator", ctx.user_ref_email(s(r, "creatorId"))),
        ("createdAt", json!(s(r, "createdAt"))),
        ("updatedAt", json!(s(r, "updatedAt"))),
        ("archivedAt", get_or_null(r, "archivedAt")),
        (
            "url",
            json!(format!(
                "https://linear.app/{}/roadmap/{}-{}",
                ctx.url_key(),
                slugify(s(r, "name")),
                s(r, "slugId")
            )),
        ),
    ]);
    // SDK model: nullable scalars are dropped.
    if let Some(m) = o.as_object_mut() {
        m.retain(|k, v| {
            !(v.is_null() && ["description", "color", "archivedAt"].contains(&k.as_str()))
        });
    }
    if projects {
        let list: Vec<Value> = r["projectIds"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| ctx.get("projects", p.as_str().unwrap_or("")))
                    .map(|p| json!({"id": s(&p, "id"), "name": s(&p, "name"), "state": s(&p, "state"), "url": ctx.project_url(&p)}))
                    .collect()
            })
            .unwrap_or_default();
        o["projects"] = Value::Array(list);
    }
    o
}

pub fn get_roadmaps(ctx: &mut Ctx, args: &Value) -> R {
    roadmaps_enabled(ctx)?;
    let list: Vec<Value> = order(args, ctx.all("roadmaps"))
        .into_iter()
        .filter(|r| archived_ok(args, r))
        .take(limit(args, 25))
        .map(|r| roadmap_out(ctx, &r, false))
        .collect();
    Ok(Value::Array(list))
}

pub fn get_roadmap(ctx: &mut Ctx, args: &Value) -> R {
    roadmaps_enabled(ctx)?;
    let r = ctx.need("roadmaps", arg(args, "id").unwrap_or(""), "Roadmap")?;
    Ok(roadmap_out(ctx, &r, true))
}

pub fn create_roadmap(ctx: &mut Ctx, args: &Value) -> R {
    roadmaps_enabled(ctx)?;
    let id = ctx.uuid("roadmap");
    let slug = ctx.slug_id("roadmap_slug");
    let now = ctx.iso_now();
    if let Some(o) = arg(args, "ownerId") {
        ctx.need("users", o, "User")?;
    }
    let doc = json!({
        "id": id, "slugId": slug, "name": s(args, "name"), "description": arg(args, "description"),
        "color": arg(args, "color"), "ownerId": arg(args, "ownerId").map(str::to_owned).unwrap_or_else(|| ctx.viewer_id()),
        "creatorId": ctx.viewer_id(), "sortOrder": args.get("sortOrder").cloned().unwrap_or(json!(0)),
        "createdAt": now, "updatedAt": now, "projectIds": [],
    });
    ctx.put("roadmaps", &id, doc.clone());
    Ok(roadmap_out(ctx, &doc, true))
}

pub fn update_roadmap(ctx: &mut Ctx, args: &Value) -> R {
    roadmaps_enabled(ctx)?;
    let mut r = ctx.need("roadmaps", arg(args, "id").unwrap_or(""), "Roadmap")?;
    let mut any = false;
    for f in ["name", "description", "color", "ownerId"] {
        if let Some(v) = arg(args, f) {
            r[f] = json!(v);
            any = true;
        }
    }
    if let Some(so) = args.get("sortOrder").filter(|x| x.is_number()) {
        r["sortOrder"] = so.clone();
        any = true;
    }
    if !any {
        return Err("At least one roadmap field must be provided".into());
    }
    r["updatedAt"] = json!(ctx.iso_now());
    ctx.save("roadmaps", r.clone());
    Ok(roadmap_out(ctx, &r, true))
}

pub fn archive_roadmap(ctx: &mut Ctx, args: &Value) -> R {
    roadmaps_enabled(ctx)?;
    let mut r = ctx.need("roadmaps", arg(args, "roadmapId").unwrap_or(""), "Roadmap")?;
    r["archivedAt"] = json!(ctx.iso_now());
    ctx.save("roadmaps", r.clone());
    Ok(json!({"success": true, "roadmap": roadmap_out(ctx, &r, false)}))
}

// ───────────────────────────── documents ─────────────────────────────

fn doc_url(ctx: &Ctx, d: &Value) -> String {
    format!(
        "https://linear.app/{}/document/{}-{}",
        ctx.url_key(),
        slugify(s(d, "title")),
        s(d, "slugId")
    )
}

pub fn document_out(ctx: &Ctx, d: &Value) -> Value {
    let release = match ctx.get("releases", s(d, "releaseId")) {
        Some(r) => {
            json!({"id": s(&r, "id"), "name": s(&r, "name"), "version": get_or_null(&r, "version")})
        }
        None => Value::Null,
    };
    let cycle = match ctx.get("cycles", s(d, "cycleId")) {
        Some(c) => {
            json!({"id": s(&c, "id"), "name": get_or_null(&c, "name"), "number": get_or_null(&c, "number")})
        }
        None => Value::Null,
    };
    obj(vec![
        ("id", json!(s(d, "id"))),
        ("title", json!(s(d, "title"))),
        ("content", get_or_null(d, "content")),
        ("color", get_or_null(d, "color")),
        ("icon", get_or_null(d, "icon")),
        ("slugId", json!(s(d, "slugId"))),
        ("sortOrder", d.get("sortOrder").cloned().unwrap_or(json!(0))),
        ("createdAt", json!(s(d, "createdAt"))),
        ("updatedAt", json!(s(d, "updatedAt"))),
        ("archivedAt", get_or_null(d, "archivedAt")),
        ("hiddenAt", get_or_null(d, "hiddenAt")),
        ("trashed", json!(d["trashed"].as_bool().unwrap_or(false))),
        ("documentContentId", get_or_null(d, "documentContentId")),
        ("url", json!(doc_url(ctx, d))),
        ("creator", ctx.user_ref_email(s(d, "creatorId"))),
        ("updatedBy", ctx.user_ref_email(s(d, "updatedById"))),
        ("project", ctx.named_ref("projects", s(d, "projectId"))),
        (
            "initiative",
            ctx.named_ref("initiatives", s(d, "initiativeId")),
        ),
        ("team", ctx.named_ref("teams", s(d, "teamId"))),
        ("issue", ctx.issue_ref(s(d, "issueId"))),
        ("release", release),
        ("cycle", cycle),
        ("resourceFolderId", get_or_null(d, "resourceFolderId")),
        (
            "lastAppliedTemplate",
            ctx.named_ref("templates", s(d, "lastAppliedTemplateId")),
        ),
    ])
}

fn documents_matching(ctx: &Ctx, args: &Value, field: Option<(&str, String)>) -> Vec<Value> {
    let title = arg(args, "title").map(str::to_lowercase);
    let list: Vec<Value> = ctx
        .all("documents")
        .into_iter()
        .filter(|d| archived_ok(args, d) && !d["trashed"].as_bool().unwrap_or(false))
        .filter(|d| field.as_ref().is_none_or(|(f, v)| s(d, f) == v))
        .filter(|d| {
            title
                .as_ref()
                .is_none_or(|t| s(d, "title").to_lowercase().contains(t.as_str()))
        })
        .collect();
    order(args, list)
        .into_iter()
        .take(limit(args, 25))
        .collect()
}

pub fn get_documents(ctx: &mut Ctx, args: &Value) -> R {
    let mut list = documents_matching(
        ctx,
        &json!({"includeArchived": args.get("includeArchived"), "title": args.get("title"), "orderBy": args.get("orderBy"), "limit": 1000}),
        None,
    );
    for f in [
        "projectId",
        "initiativeId",
        "teamId",
        "releaseId",
        "cycleId",
    ] {
        if let Some(v) = arg(args, f) {
            list.retain(|d| s(d, f) == v);
        }
    }
    if let Some(i) = arg(args, "issueId") {
        let id = ctx
            .find_issue(i)
            .map(|(_, x)| s(&x, "id").to_owned())
            .unwrap_or_else(|| i.to_owned());
        list.retain(|d| s(d, "issueId") == id);
    }
    Ok(Value::Array(
        list.iter()
            .take(limit(args, 25))
            .map(|d| document_out(ctx, d))
            .collect(),
    ))
}

pub fn get_document(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let d = ctx
        .get("documents", id)
        .or_else(|| {
            ctx.all("documents")
                .into_iter()
                .find(|d| s(d, "slugId") == id)
        })
        .ok_or_else(|| not_found("Document"))?;
    Ok(document_out(ctx, &d))
}

pub fn parent_documents(ctx: &mut Ctx, args: &Value, kind: &str) -> R {
    let (coll, field, entity) = match kind {
        "project" => ("projects", "projectId", "Project"),
        "initiative" => ("initiatives", "initiativeId", "Initiative"),
        "team" => ("teams", "teamId", "Team"),
        "release" => ("releases", "releaseId", "Release"),
        "cycle" => ("cycles", "cycleId", "Cycle"),
        _ => ("issues", "issueId", "Issue"),
    };
    let raw = arg(args, field).unwrap_or("").to_owned();
    let parent = if kind == "issue" {
        ctx.need_issue(&raw)?.1
    } else {
        ctx.need(coll, &raw, entity)?
    };
    let pid = s(&parent, "id").to_owned();
    let list = documents_matching(ctx, args, Some((field, pid)));
    let override_ref = match kind {
        "team" | "initiative" | "project" => {
            json!({"id": s(&parent, "id"), "name": s(&parent, "name")})
        }
        "release" => {
            json!({"id": s(&parent, "id"), "name": s(&parent, "name"), "version": get_or_null(&parent, "version")})
        }
        "cycle" => {
            json!({"id": s(&parent, "id"), "name": get_or_null(&parent, "name"), "number": get_or_null(&parent, "number")})
        }
        _ => {
            json!({"id": s(&parent, "id"), "identifier": s(&parent, "identifier"), "title": s(&parent, "title")})
        }
    };
    Ok(Value::Array(
        list.iter()
            .map(|d| {
                let mut o = document_out(ctx, d);
                if kind != "project" {
                    o[kind] = override_ref.clone();
                }
                o
            })
            .collect(),
    ))
}

pub fn team_resources(ctx: &mut Ctx, args: &Value) -> R {
    let team = ctx.need("teams", arg(args, "teamId").unwrap_or(""), "Team")?;
    let resources: Vec<Value> = ctx
        .all("team_resources")
        .into_iter()
        .filter(|r| s(r, "teamId") == s(&team, "id"))
        .collect();
    let sections: Vec<Value> = team["resourceSections"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let norm = |r: &Value| -> Value {
        let document = ctx.get("documents", s(r, "documentId")).map(|d| {
            json!({"id": s(&d, "id"), "title": s(&d, "title"), "url": doc_url(ctx, &d), "slugId": s(&d, "slugId"), "icon": get_or_null(&d, "icon"), "color": get_or_null(&d, "color"), "archivedAt": get_or_null(&d, "archivedAt")})
        });
        let link = r.get("externalLink").filter(|l| l.is_object()).cloned();
        let section = sections
            .iter()
            .find(|x| s(x, "id") == s(r, "sectionId"))
            .map(|x| json!({"id": s(x, "id"), "title": s(x, "title")}));
        json!({
            "id": s(r, "id"),
            "type": if document.is_some() { "document" } else if link.is_some() { "externalLink" } else { "unknown" },
            "sortOrder": r.get("sortOrder").cloned().unwrap_or(json!(0)),
            "createdAt": get_or_null(r, "createdAt"), "updatedAt": get_or_null(r, "updatedAt"),
            "section": section, "document": document, "externalLink": link,
        })
    };
    let all: Vec<Value> = resources.iter().map(norm).collect();
    let secs: Vec<Value> = sections
        .iter()
        .map(|x| {
            json!({
                "id": s(x, "id"), "title": s(x, "title"), "sortOrder": x.get("sortOrder").cloned().unwrap_or(json!(0)),
                "createdAt": get_or_null(x, "createdAt"), "updatedAt": get_or_null(x, "updatedAt"),
                "resources": all.iter().filter(|r| r["section"]["id"] == x["id"]).cloned().collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({
        "team": {"id": s(&team, "id"), "name": s(&team, "name"), "key": s(&team, "key")},
        "sections": secs,
        "unsectioned": all.iter().filter(|r| r["section"].is_null()).cloned().collect::<Vec<_>>(),
    }))
}

pub fn search_documents(ctx: &mut Ctx, args: &Value) -> R {
    let term = s(args, "term").to_lowercase();
    let words: Vec<&str> = term.split_whitespace().collect();
    let snippet = arg_i64(args, "snippetSize").unwrap_or(0).max(0) as usize;
    let list: Vec<Value> = ctx
        .all("documents")
        .into_iter()
        .filter(|d| archived_ok(args, d) && !d["trashed"].as_bool().unwrap_or(false))
        .filter(|d| arg(args, "teamId").is_none_or(|t| s(d, "teamId") == t))
        .filter(|d| {
            let hay = format!("{} {}", s(d, "title"), s(d, "content")).to_lowercase();
            !words.is_empty() && words.iter().all(|w| hay.contains(w))
        })
        .collect();
    let total = list.len();
    let nodes: Vec<Value> = order(args, list)
        .iter()
        .take(limit(args, 25))
        .map(|d| {
            let mut o = document_out(ctx, d);
            o["metadata"] = if snippet > 0 {
                json!({"snippet": s(d, "content").chars().take(snippet).collect::<String>()})
            } else {
                Value::Null
            };
            o
        })
        .collect();
    Ok(json!({"totalCount": total, "nodes": nodes}))
}

pub fn content_history(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let d = ctx
        .all("documents")
        .into_iter()
        .find(|d| s(d, "documentContentId") == id || s(d, "id") == id)
        .ok_or_else(|| not_found("DocumentContent"))?;
    let history: Vec<Value> = d["contentHistory"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|h| json!({"id": s(h, "id"), "actorIds": h.get("actorIds").cloned().unwrap_or(json!([])), "createdAt": s(h, "createdAt"), "contentDataSnapshotAt": s(h, "contentDataSnapshotAt"), "metadata": get_or_null(h, "metadata")}))
        .collect();
    Ok(json!({"success": true, "history": history}))
}

const DOC_PARENTS: [(&str, &str, &str); 6] = [
    ("projectId", "projects", "Project"),
    ("initiativeId", "initiatives", "Initiative"),
    ("teamId", "teams", "Team"),
    ("releaseId", "releases", "Release"),
    ("cycleId", "cycles", "Cycle"),
    ("lastAppliedTemplateId", "templates", "Template"),
];

pub fn create_document(ctx: &mut Ctx, args: &Value) -> R {
    let title = arg(args, "title")
        .ok_or_else(|| "Document title is required".to_owned())?
        .to_owned();
    if let Some(t) = arg(args, "teamId")
        && ctx.get("teams", t).is_none()
    {
        return Err(format!("Team with ID {t} not found"));
    }
    let id = ctx.uuid("document");
    let content_id = ctx.uuid("document_content");
    let slug = ctx.slug_id("document_slug");
    let now = ctx.iso_now();
    let mut d = json!({
        "id": id, "title": title, "content": args.get("content").cloned().unwrap_or(Value::Null),
        "icon": arg(args, "icon"), "color": arg(args, "color"), "slugId": slug,
        "sortOrder": args.get("sortOrder").cloned().unwrap_or(json!(0)), "documentContentId": content_id,
        "creatorId": ctx.viewer_id(), "updatedById": ctx.viewer_id(), "createdAt": now, "updatedAt": now,
        "resourceFolderId": arg(args, "resourceFolderId"),
    });
    for (f, coll, kind) in DOC_PARENTS {
        if let Some(v) = arg(args, f) {
            ctx.need(coll, v, kind)?;
            d[f] = json!(v);
        }
    }
    if let Some(i) = arg(args, "issueId") {
        let (_, issue) = ctx.need_issue(i)?;
        d["issueId"] = json!(s(&issue, "id"));
    }
    ctx.put("documents", &id, d.clone());
    let mut out = document_out(ctx, &d);
    if let Some(t) = arg(args, "teamId") {
        out["team"] = ctx.team_ref(t);
    }
    Ok(out)
}

pub fn update_document(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let mut d = ctx.need("documents", id, "Document")?;
    if let Some(Value::String(t)) = args.get("teamId")
        && ctx.get("teams", t).is_none()
    {
        return Err(format!("Team with ID {t} not found"));
    }
    let mut any = false;
    if let Some(t) = arg(args, "title") {
        d["title"] = json!(t);
        any = true;
    }
    if let Some(c) = args.get("content") {
        d["content"] = c.clone();
        any = true;
    }
    for f in [
        "icon",
        "color",
        "hiddenAt",
        "resourceFolderId",
        "projectId",
        "initiativeId",
        "teamId",
        "issueId",
        "releaseId",
        "cycleId",
        "lastAppliedTemplateId",
    ] {
        match args.get(f) {
            Some(Value::Null) => {
                d[f] = Value::Null;
                any = true;
            }
            Some(Value::String(v)) if !v.trim().is_empty() => {
                if let Some((_, coll, kind)) = DOC_PARENTS.iter().find(|(x, _, _)| *x == f) {
                    ctx.need(coll, v, kind)?;
                }
                d[f] = if f == "issueId" {
                    json!(s(&ctx.need_issue(v)?.1, "id"))
                } else if f == "hiddenAt" {
                    json!(crate::store::norm_time(v).unwrap_or_else(|| v.clone()))
                } else {
                    json!(v)
                };
                any = true;
            }
            _ => {}
        }
    }
    if let Some(so) = args.get("sortOrder").filter(|x| x.is_number()) {
        d["sortOrder"] = so.clone();
        any = true;
    }
    if let Some(t) = arg_bool(args, "trashed") {
        d["trashed"] = json!(t);
        any = true;
    }
    if !any {
        return Err("At least one document field must be provided".into());
    }
    d["updatedAt"] = json!(ctx.iso_now());
    d["updatedById"] = json!(ctx.viewer_id());
    ctx.save("documents", d.clone());
    let mut out = document_out(ctx, &d);
    match args.get("teamId") {
        Some(Value::String(t)) => out["team"] = ctx.team_ref(t),
        Some(Value::Null) => out["team"] = Value::Null,
        _ => {}
    }
    Ok(out)
}

pub fn archive_document(ctx: &mut Ctx, args: &Value, archive: bool) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    let mut d = ctx.need("documents", &id, "Document")?;
    d["archivedAt"] = if archive {
        json!(ctx.iso_now())
    } else {
        Value::Null
    };
    ctx.save("documents", d);
    Ok(json!({"success": true, "id": id}))
}
