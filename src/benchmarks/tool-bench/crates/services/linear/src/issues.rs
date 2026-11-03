//! Issues, comments, labels on issues, relations, history, attachments, templates and
//! custom fields.

use serde_json::{Value, json};

use crate::store::{
    Ctx, R, arg, arg_bool, arg_i64, arg_strs, get_or_null, iso, norm_time, not_found, obj, opt,
    parse_time, s, sdk,
};

// ───────────────────────────── representations ─────────────────────────────

fn state_obj(ctx: &Ctx, id: &str) -> Value {
    match ctx.get("workflow_states", id) {
        Some(st) => sdk(vec![
            ("id", json!(s(&st, "id"))),
            ("name", json!(s(&st, "name"))),
            ("color", get_or_null(&st, "color")),
            ("type", json!(s(&st, "type"))),
        ]),
        None => Value::Null,
    }
}

fn labels_of(ctx: &Ctx, issue: &Value) -> Vec<Value> {
    issue["labelIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|id| ctx.get("labels", id.as_str().unwrap_or("")))
                .map(|l| {
                    sdk(vec![
                        ("id", json!(s(&l, "id"))),
                        ("name", json!(s(&l, "name"))),
                        ("color", get_or_null(&l, "color")),
                    ])
                })
                .collect()
        })
        .unwrap_or_default()
}

fn sdk_named(ctx: &Ctx, coll: &str, id: &str) -> Value {
    match ctx.get(coll, id) {
        Some(x) => sdk(vec![
            ("id", json!(s(&x, "id"))),
            ("name", get_or_null(&x, "name")),
        ]),
        None => Value::Null,
    }
}

/// `getIssues` / `getIssueById` / `searchIssues` shape (SDK models; no identifier).
pub fn issue_full(ctx: &Ctx, i: &Value, with_comments: bool) -> Value {
    let parent = match ctx.find_issue(s(i, "parentId")) {
        Some((_, p)) if !s(i, "parentId").is_empty() => {
            json!({"id": s(&p, "id"), "title": s(&p, "title")})
        }
        _ => Value::Null,
    };
    let mut fields = vec![
        ("id", json!(s(i, "id"))),
        ("title", json!(s(i, "title"))),
        ("description", get_or_null(i, "description")),
        ("state", state_obj(ctx, s(i, "stateId"))),
        (
            "priority",
            json!(i.get("priority").and_then(Value::as_i64).unwrap_or(0)),
        ),
        ("estimate", get_or_null(i, "estimate")),
        ("dueDate", get_or_null(i, "dueDate")),
        ("team", ctx.team_ref(s(i, "teamId"))),
        ("assignee", ctx.user_ref(s(i, "assigneeId"))),
        ("project", sdk_named(ctx, "projects", s(i, "projectId"))),
        ("cycle", sdk_named(ctx, "cycles", s(i, "cycleId"))),
        (
            "projectMilestone",
            sdk_named(ctx, "milestones", s(i, "projectMilestoneId")),
        ),
        ("parent", parent),
        ("labels", Value::Array(labels_of(ctx, i))),
        (
            "sortOrder",
            json!(i.get("sortOrder").cloned().unwrap_or(json!(0))),
        ),
        ("createdAt", json!(s(i, "createdAt"))),
        ("updatedAt", json!(s(i, "updatedAt"))),
        ("url", json!(ctx.issue_url(i))),
    ];
    // The SDK keeps nulls for these relation fields (`x ? {...} : null`).
    let keep_null = [
        "state",
        "team",
        "assignee",
        "project",
        "cycle",
        "projectMilestone",
        "parent",
    ];
    let mut m = serde_json::Map::new();
    for (k, v) in fields.drain(..) {
        if !v.is_null() || keep_null.contains(&k) {
            m.insert(k.to_owned(), v);
        }
    }
    if with_comments {
        let comments: Vec<Value> = issue_comments(ctx, s(i, "id"))
            .iter()
            .map(|c| {
                json!({"id": s(c, "id"), "body": s(c, "body"), "createdAt": s(c, "createdAt"), "user": ctx.user_ref(s(c, "userId"))})
            })
            .collect();
        m.insert("comments".into(), Value::Array(comments));
    }
    Value::Object(m)
}

/// Raw GraphQL `ISSUE_SUMMARY_FIELDS` node (nulls kept).
pub fn issue_summary(ctx: &Ctx, i: &Value) -> Value {
    let named = |coll: &str, id: &str| match ctx.get(coll, id) {
        Some(x) => json!({"id": s(&x, "id"), "name": get_or_null(&x, "name")}),
        None => Value::Null,
    };
    let state = match ctx.get("workflow_states", s(i, "stateId")) {
        Some(st) => {
            json!({"id": s(&st, "id"), "name": s(&st, "name"), "color": get_or_null(&st, "color"), "type": s(&st, "type")})
        }
        None => Value::Null,
    };
    let labels: Vec<Value> = labels_of(ctx, i)
        .into_iter()
        .take(10)
        .map(|l| json!({"id": l["id"], "name": l["name"], "color": get_or_null(&l, "color")}))
        .collect();
    obj(vec![
        ("id", json!(s(i, "id"))),
        ("identifier", json!(s(i, "identifier"))),
        ("title", json!(s(i, "title"))),
        ("description", get_or_null(i, "description")),
        ("state", state),
        (
            "priority",
            json!(i.get("priority").and_then(Value::as_i64).unwrap_or(0)),
        ),
        ("estimate", get_or_null(i, "estimate")),
        ("dueDate", get_or_null(i, "dueDate")),
        ("team", ctx.team_ref(s(i, "teamId"))),
        ("assignee", ctx.user_ref(s(i, "assigneeId"))),
        ("cycle", named("cycles", s(i, "cycleId"))),
        (
            "projectMilestone",
            named("milestones", s(i, "projectMilestoneId")),
        ),
        ("labels", Value::Array(labels)),
        ("sortOrder", i.get("sortOrder").cloned().unwrap_or(json!(0))),
        ("createdAt", json!(s(i, "createdAt"))),
        ("updatedAt", json!(s(i, "updatedAt"))),
        ("url", json!(ctx.issue_url(i))),
    ])
}

fn brief(ctx: &Ctx, i: &Value) -> Value {
    sdk(vec![
        ("id", json!(s(i, "id"))),
        ("title", json!(s(i, "title"))),
        ("description", get_or_null(i, "description")),
        ("url", json!(ctx.issue_url(i))),
    ])
}

pub fn visible_issues(ctx: &Ctx) -> Vec<Value> {
    let mut v: Vec<Value> = ctx
        .all("issues")
        .into_iter()
        .filter(|i| {
            opt(i, "archivedAt").is_none() && opt(i, "trashed").is_none_or(|t| t == json!(false))
        })
        .collect();
    v.sort_by(|a, b| s(b, "createdAt").cmp(s(a, "createdAt")));
    v
}

fn state_type(ctx: &Ctx, i: &Value) -> String {
    ctx.get("workflow_states", s(i, "stateId"))
        .map(|st| s(&st, "type").to_owned())
        .unwrap_or_default()
}

// ───────────────────────────── queries ─────────────────────────────

pub fn get_issues(ctx: &mut Ctx, args: &Value) -> R {
    let limit = arg_i64(args, "limit").unwrap_or(25).max(0) as usize;
    Ok(Value::Array(
        visible_issues(ctx)
            .iter()
            .take(limit)
            .map(|i| issue_full(ctx, i, false))
            .collect(),
    ))
}

pub fn get_issue_by_id(ctx: &mut Ctx, args: &Value) -> R {
    let (_, i) = ctx.need_issue(arg(args, "id").unwrap_or(""))?;
    Ok(issue_full(ctx, &i, true))
}

/// ISO 8601 duration (optionally negative) in seconds, approximating months/years.
fn duration_secs(text: &str) -> Option<i64> {
    let (neg, rest) = match text.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, text),
    };
    let rest = rest.strip_prefix('P')?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut total = 0i64;
    let mut num = String::new();
    for c in date.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            let n: i64 = num.parse().ok()?;
            num.clear();
            total += n * match c {
                'Y' => 365 * 86_400,
                'M' => 30 * 86_400,
                'W' => 7 * 86_400,
                'D' => 86_400,
                _ => return None,
            };
        }
    }
    for c in time.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            let n: i64 = num.parse().ok()?;
            num.clear();
            total += n * match c {
                'H' => 3600,
                'M' => 60,
                'S' => 1,
                _ => return None,
            };
        }
    }
    Some(if neg { -total } else { total })
}

fn bound(ctx: &Ctx, text: &str) -> Option<i64> {
    if text.starts_with('P') || text.starts_with("-P") {
        return duration_secs(text).map(|d| ctx.now + d);
    }
    parse_time(text)
}

fn dates_match(ctx: &Ctx, i: &Value, dates: &Value) -> Result<bool, String> {
    let Some(map) = dates.as_object() else {
        return Err("Invalid arguments for searchIssues".into());
    };
    for (field, range) in map {
        let key = match field.as_str() {
            "due" => "dueDate",
            "created" => "createdAt",
            "updated" => "updatedAt",
            "started" => "startedAt",
            "completed" => "completedAt",
            "autoClosed" => "autoClosedAt",
            "triaged" => "triagedAt",
            _ => return Err(format!("dates has unknown field: {field}")),
        };
        let Some(v) = opt(i, key).and_then(|x| x.as_str().and_then(parse_time)) else {
            let has_bound = ["after", "before", "on"]
                .iter()
                .any(|b| arg(range, b).is_some());
            if has_bound {
                return Ok(false);
            }
            continue;
        };
        if let Some(on) = arg(range, "on") {
            let start = parse_time(on)
                .ok_or_else(|| format!("dates.{field} on must be a YYYY-MM-DD date"))?;
            if !(v >= start && v < start + 86_400) {
                return Ok(false);
            }
        }
        if let Some(a) = arg(range, "after") {
            let b = bound(ctx, a).ok_or_else(|| {
                format!("dates.{field} after must be a YYYY-MM-DD date or ISO 8601 duration")
            })?;
            if v < b {
                return Ok(false);
            }
        }
        if let Some(bf) = arg(range, "before") {
            let b = bound(ctx, bf).ok_or_else(|| {
                format!("dates.{field} before must be a YYYY-MM-DD date or ISO 8601 duration")
            })?;
            if v >= b {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub fn search_issues(ctx: &mut Ctx, args: &Value) -> R {
    let limit = match arg_i64(args, "limit") {
        Some(0) | None => 10,
        Some(n) => n as usize,
    };
    let mut state_ids: Vec<String> = Vec::new();
    if let Some(names) = arg_strs(args, "states").filter(|n| !n.is_empty()) {
        let teams: Vec<String> = match arg(args, "teamId") {
            Some(t) => vec![t.to_owned()],
            None => ctx
                .all("teams")
                .iter()
                .map(|t| s(t, "id").to_owned())
                .collect(),
        };
        for t in teams {
            for st in ctx.all("workflow_states") {
                if s(&st, "teamId") == t
                    && opt(&st, "archivedAt").is_none()
                    && names
                        .iter()
                        .any(|n| n.to_lowercase() == s(&st, "name").to_lowercase())
                {
                    state_ids.push(s(&st, "id").to_owned());
                }
            }
        }
    }
    let query = arg(args, "query").map(str::to_owned);
    let mut out = Vec::new();
    for i in visible_issues(ctx) {
        if let Some(t) = arg(args, "teamId")
            && s(&i, "teamId") != t
        {
            continue;
        }
        if let Some(a) = arg(args, "assigneeId")
            && s(&i, "assigneeId") != a
        {
            continue;
        }
        if let Some(p) = arg(args, "projectId")
            && s(&i, "projectId") != p
        {
            continue;
        }
        if !state_ids.is_empty() && !state_ids.iter().any(|x| x == s(&i, "stateId")) {
            continue;
        }
        if let Some(d) = args.get("dates")
            && !d.is_null()
            && !dates_match(ctx, &i, d)?
        {
            continue;
        }
        if let Some(q) = &query
            && !(s(&i, "title").contains(q.as_str()) || s(&i, "description").contains(q.as_str()))
        {
            continue;
        }
        out.push(issue_full(ctx, &i, false));
        if out.len() == limit {
            break;
        }
    }
    Ok(Value::Array(out))
}

/// Shared filter for project and cycle issue listings (`buildPMIssueFilter`).
pub fn pm_filter(ctx: &Ctx, args: &Value, i: &Value) -> bool {
    if let Some(a) = arg(args, "assigneeId")
        && s(i, "assigneeId") != a
    {
        return false;
    }
    if let Some(labels) = arg_strs(args, "labelIds").filter(|l| !l.is_empty())
        && !i["labelIds"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| labels.iter().any(|l| x == l)))
    {
        return false;
    }
    if let Some(c) = arg(args, "cycleId")
        && s(i, "cycleId") != c
    {
        return false;
    }
    if let Some(m) = arg(args, "projectMilestoneId")
        && s(i, "projectMilestoneId") != m
    {
        return false;
    }
    if arg_bool(args, "includeCompleted") == Some(false) && opt(i, "completedAt").is_some() {
        return false;
    }
    if let Some(states) = arg_strs(args, "states").filter(|l| !l.is_empty()) {
        let name = ctx
            .get("workflow_states", s(i, "stateId"))
            .map(|st| s(&st, "name").to_owned())
            .unwrap_or_default();
        if !states.contains(&name) {
            return false;
        }
    }
    true
}

pub fn order(args: &Value, mut v: Vec<Value>) -> Vec<Value> {
    if arg(args, "orderBy") == Some("updatedAt") {
        v.sort_by(|a, b| s(b, "updatedAt").cmp(s(a, "updatedAt")));
    } else {
        v.sort_by(|a, b| s(b, "createdAt").cmp(s(a, "createdAt")));
    }
    v
}

// ───────────────────────────── mutations ─────────────────────────────

fn check_ref(ctx: &Ctx, coll: &str, id: Option<&str>, kind: &str) -> Result<(), String> {
    match id {
        Some(x) if ctx.get(coll, x).is_none() => Err(not_found(kind)),
        _ => Ok(()),
    }
}

fn check_priority(args: &Value) -> Result<(), String> {
    if let Some(p) = arg_i64(args, "priority")
        && !(0..=4).contains(&p)
    {
        return Err(format!(
            "Argument Validation Error - priority must not be {} than {}.",
            if p > 4 { "greater" } else { "less" },
            if p > 4 { 4 } else { 0 }
        ));
    }
    Ok(())
}

fn default_state(ctx: &Ctx, team: &Value) -> String {
    if let Some(d) = team.get("defaultIssueStateId").and_then(Value::as_str) {
        return d.to_owned();
    }
    let states: Vec<Value> = ctx
        .all("workflow_states")
        .into_iter()
        .filter(|st| s(st, "teamId") == s(team, "id"))
        .collect();
    for ty in ["unstarted", "backlog", "triage", "started"] {
        if let Some(st) = states.iter().find(|st| s(st, "type") == ty) {
            return s(st, "id").to_owned();
        }
    }
    states
        .first()
        .map(|st| s(st, "id").to_owned())
        .unwrap_or_default()
}

/// Updates state-derived timestamps when the state changes.
fn apply_state(ctx: &Ctx, issue: &mut Value, state_id: &str) {
    issue["stateId"] = json!(state_id);
    let ty = ctx
        .get("workflow_states", state_id)
        .map(|st| s(&st, "type").to_owned())
        .unwrap_or_default();
    let now = ctx.iso_now();
    match ty.as_str() {
        "completed" => {
            issue["completedAt"] = json!(now);
            issue["canceledAt"] = Value::Null;
        }
        "canceled" => {
            issue["canceledAt"] = json!(now);
            issue["completedAt"] = Value::Null;
        }
        "started" => {
            if opt(issue, "startedAt").is_none() {
                issue["startedAt"] = json!(now);
            }
            issue["completedAt"] = Value::Null;
            issue["canceledAt"] = Value::Null;
        }
        _ => {
            issue["completedAt"] = Value::Null;
            issue["canceledAt"] = Value::Null;
        }
    }
}

pub fn create_issue_inner(ctx: &mut Ctx, args: &Value) -> Result<Value, String> {
    let team_id = arg(args, "teamId")
        .ok_or_else(|| "Argument Validation Error - teamId must be a UUID.".to_owned())?;
    let team = ctx.need("teams", team_id, "Team")?;
    let title = s(args, "title").to_owned();
    check_priority(args)?;
    check_ref(ctx, "users", arg(args, "assigneeId"), "User")?;
    check_ref(ctx, "projects", arg(args, "projectId"), "Project")?;
    check_ref(
        ctx,
        "milestones",
        arg(args, "projectMilestoneId"),
        "ProjectMilestone",
    )?;
    check_ref(ctx, "cycles", arg(args, "cycleId"), "Cycle")?;
    check_ref(
        ctx,
        "workflow_states",
        arg(args, "stateId"),
        "WorkflowState",
    )?;
    if let Some(p) = arg(args, "parentId")
        && ctx.find_issue(p).is_none()
    {
        return Err(not_found("Issue"));
    }
    let mut label_ids = arg_strs(args, "labelIds").unwrap_or_default();
    for l in &label_ids {
        check_ref(ctx, "labels", Some(l), "IssueLabel")?;
    }
    let template = match arg(args, "templateId") {
        Some(t) => Some(ctx.need("templates", t, "Template")?),
        None => None,
    };
    let tdata = template
        .as_ref()
        .map(|t| t["templateData"].clone())
        .unwrap_or(Value::Null);
    if label_ids.is_empty()
        && let Some(tl) = tdata["labelIds"].as_array()
    {
        label_ids = tl
            .iter()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect();
    }
    let key = s(&team, "key").to_owned();
    let number = ctx
        .all("issues")
        .iter()
        .filter(|i| s(i, "teamId") == s(&team, "id"))
        .filter_map(|i| i["number"].as_i64())
        .max()
        .unwrap_or(0)
        + 1;
    let identifier = format!("{key}-{number}");
    let id = ctx.uuid("issue");
    let state = arg(args, "stateId")
        .map(str::to_owned)
        .or_else(|| tdata["stateId"].as_str().map(str::to_owned))
        .unwrap_or_else(|| default_state(ctx, &team));
    let min_sort = ctx
        .all("issues")
        .iter()
        .filter_map(|i| i["sortOrder"].as_f64())
        .fold(0.0_f64, f64::min);
    let now = ctx.iso_now();
    let viewer = ctx.viewer_id();
    let mut subscribers = arg_strs(args, "subscriberIds").unwrap_or_default();
    if !subscribers.contains(&viewer) {
        subscribers.insert(0, viewer.clone());
    }
    let description = args
        .get("description")
        .filter(|d| d.is_string())
        .cloned()
        .or_else(|| opt(&tdata, "description"))
        .unwrap_or(Value::Null);
    let priority = arg_i64(args, "priority")
        .or_else(|| tdata["priority"].as_i64())
        .unwrap_or(0);
    let mut issue = json!({
        "id": id,
        "identifier": identifier,
        "number": number,
        "title": title,
        "description": description,
        "priority": priority,
        "estimate": args.get("estimate").cloned().unwrap_or(Value::Null),
        "dueDate": arg(args, "dueDate").map(|d| json!(d)).unwrap_or(Value::Null),
        "sortOrder": args.get("sortOrder").cloned().unwrap_or(json!(min_sort - 1000.0)),
        "teamId": s(&team, "id"),
        "stateId": state,
        "assigneeId": arg(args, "assigneeId"),
        "creatorId": viewer,
        "projectId": arg(args, "projectId"),
        "projectMilestoneId": arg(args, "projectMilestoneId"),
        "cycleId": arg(args, "cycleId"),
        "parentId": arg(args, "parentId").and_then(|p| ctx.find_issue(p)).map(|(_, p)| p["id"].clone()),
        "labelIds": label_ids,
        "subscriberIds": subscribers,
        "createdAt": now,
        "updatedAt": now,
        "startedAt": null,
        "completedAt": null,
        "canceledAt": null,
        "archivedAt": null,
    });
    let st = s(&issue, "stateId").to_owned();
    apply_state(ctx, &mut issue, &st);
    ctx.put("issues", &identifier, issue.clone());
    Ok(issue)
}

pub fn create_issue(ctx: &mut Ctx, args: &Value) -> R {
    let issue = create_issue_inner(ctx, args)?;
    Ok(brief(ctx, &issue))
}

pub fn update_issue(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let (key, mut issue) = ctx.need_issue(id)?;
    check_priority(args)?;
    let issue_id = s(&issue, "id").to_owned();
    let mut changed = false;
    if let Some(t) = args.get("title").and_then(Value::as_str) {
        if t.trim().is_empty() {
            return Err("Argument Validation Error - title should not be empty.".into());
        }
        ctx.history(&issue_id, "title", json!(s(&issue, "title")), json!(t));
        issue["title"] = json!(t);
        changed = true;
    }
    if let Some(d) = args.get("description").and_then(Value::as_str) {
        issue["description"] = json!(d);
        changed = true;
    }
    if let Some(team_id) = arg(args, "teamId")
        && team_id != s(&issue, "teamId")
    {
        transfer(ctx, &mut issue, team_id)?;
        changed = true;
    }
    if let Some(st) = arg(args, "stateId") {
        let state = ctx.need("workflow_states", st, "WorkflowState")?;
        if s(&state, "teamId") != s(&issue, "teamId") {
            return Err(
                "Invalid input - The workflow state must belong to the issue's team.".into(),
            );
        }
        ctx.history(&issue_id, "state", json!(s(&issue, "stateId")), json!(st));
        apply_state(ctx, &mut issue, st);
        changed = true;
    }
    if let Some(p) = arg_i64(args, "priority") {
        issue["priority"] = json!(p);
        changed = true;
    }
    for (field, coll, kind) in [
        ("projectId", "projects", "Project"),
        ("projectMilestoneId", "milestones", "ProjectMilestone"),
        ("assigneeId", "users", "User"),
        ("cycleId", "cycles", "Cycle"),
    ] {
        if let Some(v) = arg(args, field) {
            let target = ctx.need(coll, v, kind)?;
            if field == "cycleId" && s(&target, "teamId") != s(&issue, "teamId") {
                return Err("Invalid input - The cycle must belong to the issue's team.".into());
            }
            ctx.history(
                &issue_id,
                field,
                get_or_null(&issue, field),
                json!(s(&target, "id")),
            );
            issue[field] = json!(s(&target, "id"));
            changed = true;
        }
    }
    if let Some(e) = args.get("estimate").filter(|e| e.is_number()) {
        issue["estimate"] = e.clone();
        changed = true;
    }
    if let Some(d) = arg(args, "dueDate") {
        issue["dueDate"] = json!(d);
        changed = true;
    }
    if let Some(p) = arg(args, "parentId") {
        let (_, parent) = ctx.need_issue(p)?;
        issue["parentId"] = parent["id"].clone();
        changed = true;
    }
    if let Some(sub) = arg_strs(args, "subscriberIds").filter(|x| !x.is_empty()) {
        issue["subscriberIds"] = json!(sub);
        changed = true;
    }
    if let Some(so) = args.get("sortOrder").filter(|x| x.is_number()) {
        issue["sortOrder"] = so.clone();
        changed = true;
    }
    let added = arg_strs(args, "addedLabelIds").filter(|x| !x.is_empty());
    let removed = arg_strs(args, "removedLabelIds").filter(|x| !x.is_empty());
    let mut labels: Vec<String> = issue["labelIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if added.is_some() || removed.is_some() {
        for l in added.unwrap_or_default() {
            ctx.need("labels", &l, "IssueLabel")?;
            if !labels.contains(&l) {
                labels.push(l);
            }
        }
        let removed = removed.unwrap_or_default();
        labels.retain(|l| !removed.contains(l));
        issue["labelIds"] = json!(labels);
        changed = true;
    } else if let Some(set) = arg_strs(args, "labelIds").filter(|x| !x.is_empty()) {
        for l in &set {
            ctx.need("labels", l, "IssueLabel")?;
        }
        issue["labelIds"] = json!(set);
        changed = true;
    }
    if changed {
        issue["updatedAt"] = json!(ctx.iso_now());
    }
    ctx.put("issues", &key, issue.clone());
    Ok(brief(ctx, &issue))
}

/// Moves an issue to another team: new number/identifier, matching state, no cycle.
fn transfer(ctx: &mut Ctx, issue: &mut Value, team_id: &str) -> Result<(), String> {
    let team = ctx.need("teams", team_id, "Team")?;
    let number = ctx
        .all("issues")
        .iter()
        .filter(|i| s(i, "teamId") == s(&team, "id"))
        .filter_map(|i| i["number"].as_i64())
        .max()
        .unwrap_or(0)
        + 1;
    let old_state = ctx
        .get("workflow_states", s(issue, "stateId"))
        .unwrap_or(Value::Null);
    let states: Vec<Value> = ctx
        .all("workflow_states")
        .into_iter()
        .filter(|st| s(st, "teamId") == s(&team, "id"))
        .collect();
    let new_state = states
        .iter()
        .find(|st| s(st, "name") == s(&old_state, "name"))
        .or_else(|| {
            states
                .iter()
                .find(|st| s(st, "type") == s(&old_state, "type"))
        })
        .map(|st| s(st, "id").to_owned())
        .unwrap_or_else(|| default_state(ctx, &team));
    issue["previousIdentifiers"] = json!([s(issue, "identifier")]);
    issue["teamId"] = json!(s(&team, "id"));
    issue["number"] = json!(number);
    issue["identifier"] = json!(format!("{}-{number}", s(&team, "key")));
    issue["cycleId"] = Value::Null;
    apply_state(ctx, issue, &new_state);
    Ok(())
}

pub fn assign_issue(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let assignee = arg(args, "assigneeId").unwrap_or("");
    let user = ctx.need("users", assignee, "User")?;
    ctx.history(
        s(&issue, "id"),
        "assignee",
        get_or_null(&issue, "assigneeId"),
        json!(s(&user, "id")),
    );
    issue["assigneeId"] = json!(s(&user, "id"));
    issue["updatedAt"] = json!(ctx.iso_now());
    ctx.put("issues", &key, issue.clone());
    Ok(json!({
        "success": true,
        "issue": {
            "id": s(&issue, "id"),
            "identifier": s(&issue, "identifier"),
            "title": s(&issue, "title"),
            "assignee": {"id": s(&user, "id"), "name": s(&user, "name"), "displayName": s(&user, "displayName")},
            "url": ctx.issue_url(&issue),
        }
    }))
}

pub fn subscribe(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let viewer = ctx.viewer_id();
    let user = ctx.get("users", &viewer).unwrap_or(Value::Null);
    let subs = issue
        .as_object_mut()
        .expect("issue")
        .entry("subscriberIds")
        .or_insert_with(|| json!([]));
    if !subs
        .as_array()
        .is_some_and(|a| a.iter().any(|x| x == viewer.as_str()))
    {
        subs.as_array_mut().expect("array").push(json!(viewer));
    }
    ctx.put("issues", &key, issue.clone());
    Ok(json!({
        "success": true,
        "message": format!("User {} ({viewer}) is subscribed to issue {}.", s(&user, "name"), s(&issue, "identifier")),
    }))
}

pub fn convert_to_subtask(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let (_, parent) = ctx.need_issue(arg(args, "parentIssueId").unwrap_or(""))?;
    if s(&parent, "id") == s(&issue, "id") {
        return Err("Invalid input - An issue cannot be its own parent.".into());
    }
    issue["parentId"] = json!(s(&parent, "id"));
    issue["updatedAt"] = json!(ctx.iso_now());
    ctx.put("issues", &key, issue.clone());
    Ok(json!({
        "success": true,
        "issue": {
            "id": s(&issue, "id"), "identifier": s(&issue, "identifier"), "title": s(&issue, "title"),
            "parent": {"id": s(&parent, "id"), "identifier": s(&parent, "identifier"), "title": s(&parent, "title")},
            "url": ctx.issue_url(&issue),
        }
    }))
}

pub fn create_relation(ctx: &mut Ctx, args: &Value) -> R {
    let a = arg(args, "issueId").unwrap_or("").to_owned();
    let b = arg(args, "relatedIssueId").unwrap_or("").to_owned();
    let ty = arg(args, "type").unwrap_or("");
    let (src, dst, canonical) = match ty {
        "blocks" | "duplicate" | "related" => (a, b, ty),
        "blocked_by" => (b, a, "blocks"),
        "duplicate_of" => (b, a, "duplicate"),
        other => return Err(format!("{other} is not a valid relation type")),
    };
    let (_, si) = ctx.need_issue(&src)?;
    let (_, ti) = ctx.need_issue(&dst)?;
    let id = ctx.uuid("relation");
    ctx.put(
        "issue_relations",
        &id,
        json!({"id": id, "type": canonical, "issueId": s(&si, "id"), "relatedIssueId": s(&ti, "id"), "createdAt": ctx.iso_now()}),
    );
    Ok(json!({
        "success": true,
        "relation": {"id": id, "type": canonical, "issueIdentifier": s(&si, "identifier"), "relatedIssueIdentifier": s(&ti, "identifier")},
    }))
}

pub fn delete_relation(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    ctx.remove("issue_relations", id)
        .ok_or_else(|| not_found("IssueRelation"))?;
    Ok(json!({"success": true, "id": id}))
}

pub fn archive_issue(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    issue["archivedAt"] = json!(ctx.iso_now());
    ctx.put("issues", &key, issue.clone());
    Ok(
        json!({"success": true, "message": format!("Issue {} has been archived", s(&issue, "identifier"))}),
    )
}

pub fn set_priority(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    check_priority(args)?;
    let p = arg_i64(args, "priority").unwrap_or(0);
    ctx.history(
        s(&issue, "id"),
        "priority",
        get_or_null(&issue, "priority"),
        json!(p),
    );
    issue["priority"] = json!(p);
    issue["updatedAt"] = json!(ctx.iso_now());
    ctx.put("issues", &key, issue.clone());
    Ok(json!({"success": true, "issue": {
        "id": s(&issue, "id"), "identifier": s(&issue, "identifier"), "title": s(&issue, "title"),
        "priority": p, "url": ctx.issue_url(&issue),
    }}))
}

pub fn transfer_issue(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let team_id = arg(args, "teamId").unwrap_or("");
    ctx.need("teams", team_id, "Team")?;
    if team_id != s(&issue, "teamId") {
        transfer(ctx, &mut issue, team_id)?;
        issue["updatedAt"] = json!(ctx.iso_now());
    }
    ctx.put("issues", &key, issue.clone());
    Ok(json!({"success": true, "issue": {
        "id": s(&issue, "id"), "identifier": s(&issue, "identifier"), "title": s(&issue, "title"),
        "team": ctx.team_ref_key(s(&issue, "teamId")), "url": ctx.issue_url(&issue),
    }}))
}

pub fn duplicate_issue(ctx: &mut Ctx, args: &Value) -> R {
    let (_, issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let mut a =
        json!({"title": format!("{} (Copy)", s(&issue, "title")), "teamId": s(&issue, "teamId")});
    if let Some(d) = opt(&issue, "description") {
        a["description"] = d;
    }
    let copy = create_issue_inner(ctx, &a)?;
    Ok(json!({
        "success": true,
        "originalIssue": {"id": s(&issue, "id"), "identifier": s(&issue, "identifier"), "title": s(&issue, "title")},
        "duplicatedIssue": {"id": s(&copy, "id"), "identifier": s(&copy, "identifier"), "title": s(&copy, "title"), "url": ctx.issue_url(&copy)},
    }))
}

pub fn issue_history(ctx: &mut Ctx, args: &Value) -> R {
    let (_, issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let limit = arg_i64(args, "limit").unwrap_or(10).max(0) as usize;
    let events: Vec<Value> = ctx
        .all("issue_history")
        .into_iter()
        .filter(|h| s(h, "issueId") == s(&issue, "id"))
        .take(limit)
        .map(|h| {
            let actor = match ctx.get("users", s(&h, "actorId")) {
                Some(u) => sdk(vec![("id", json!(s(&u, "id"))), ("name", json!(s(&u, "name"))), ("displayName", get_or_null(&u, "displayName"))]),
                None => Value::Null,
            };
            // The SDK's IssueHistory model has no `type`/`from`/`to`, so the server always
            // reports "unknown" and nulls.
            json!({"id": s(&h, "id"), "createdAt": s(&h, "createdAt"), "actor": actor, "type": "unknown", "from": null, "to": null})
        })
        .collect();
    Ok(
        json!({"issueId": s(&issue, "id"), "identifier": s(&issue, "identifier"), "history": events}),
    )
}

pub fn add_label(ctx: &mut Ctx, args: &Value, add: bool) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let label = arg(args, "labelId").unwrap_or("").to_owned();
    let mut labels: Vec<String> = issue["labelIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if add {
        if !labels.contains(&label) {
            ctx.need("labels", &label, "IssueLabel")?;
            labels.push(label.clone());
            issue["updatedAt"] = json!(ctx.iso_now());
        }
    } else if labels.contains(&label) {
        labels.retain(|l| *l != label);
        issue["updatedAt"] = json!(ctx.iso_now());
    }
    issue["labelIds"] = json!(labels);
    ctx.put("issues", &key, issue.clone());
    Ok(json!({"success": true, "issueId": s(&issue, "id"), "labelId": label}))
}

// ───────────────────────────── comments ─────────────────────────────

pub fn issue_comments(ctx: &Ctx, issue_id: &str) -> Vec<Value> {
    let mut v: Vec<Value> = ctx
        .all("comments")
        .into_iter()
        .filter(|c| s(c, "issueId") == issue_id && opt(c, "archivedAt").is_none())
        .collect();
    v.sort_by(|a, b| s(a, "createdAt").cmp(s(b, "createdAt")));
    v
}

pub fn comment_out(ctx: &Ctx, c: &Value) -> Value {
    let issue = ctx.issue_ref(s(c, "issueId"));
    let url = match ctx.find_issue(s(c, "issueId")) {
        Some((_, i)) if !s(c, "issueId").is_empty() => format!(
            "{}#comment-{}",
            ctx.issue_url(&i),
            &s(c, "id")[..8.min(s(c, "id").len())]
        ),
        _ => format!(
            "https://linear.app/{}/comment/{}",
            ctx.url_key(),
            s(c, "id")
        ),
    };
    let update_ref = |coll: &str, id: &str| match ctx.get(coll, id) {
        Some(u) => json!({"id": s(&u, "id"), "body": get_or_null(&u, "body")}),
        None => Value::Null,
    };
    let user = match ctx.get("users", s(c, "userId")) {
        Some(u) => sdk(vec![
            ("id", json!(s(&u, "id"))),
            ("name", json!(s(&u, "name"))),
            ("displayName", get_or_null(&u, "displayName")),
            ("email", get_or_null(&u, "email")),
        ]),
        None => Value::Null,
    };
    obj(vec![
        ("id", json!(s(c, "id"))),
        ("body", json!(s(c, "body"))),
        ("createdAt", json!(s(c, "createdAt"))),
        (
            "updatedAt",
            json!(match s(c, "updatedAt") {
                "" => s(c, "createdAt"),
                u => u,
            }),
        ),
        ("editedAt", get_or_null(c, "editedAt")),
        ("archivedAt", get_or_null(c, "archivedAt")),
        ("resolvedAt", get_or_null(c, "resolvedAt")),
        ("quotedText", get_or_null(c, "quotedText")),
        ("documentContentId", get_or_null(c, "documentContentId")),
        ("initiativeId", get_or_null(c, "initiativeId")),
        ("initiativeUpdateId", get_or_null(c, "initiativeUpdateId")),
        ("issueId", get_or_null(c, "issueId")),
        ("parentId", get_or_null(c, "parentId")),
        ("projectId", get_or_null(c, "projectId")),
        ("projectUpdateId", get_or_null(c, "projectUpdateId")),
        ("resolvingCommentId", get_or_null(c, "resolvingCommentId")),
        ("url", json!(url)),
        ("issue", issue),
        ("project", ctx.named_ref("projects", s(c, "projectId"))),
        (
            "initiative",
            ctx.named_ref("initiatives", s(c, "initiativeId")),
        ),
        (
            "projectUpdate",
            update_ref("project_updates", s(c, "projectUpdateId")),
        ),
        (
            "initiativeUpdate",
            update_ref("initiative_updates", s(c, "initiativeUpdateId")),
        ),
        (
            "parent",
            if s(c, "parentId").is_empty() {
                Value::Null
            } else {
                json!({"id": s(c, "parentId")})
            },
        ),
        ("user", user),
    ])
}

const TARGETS: [&str; 7] = [
    "issueId",
    "projectId",
    "initiativeId",
    "projectUpdateId",
    "initiativeUpdateId",
    "documentContentId",
    "postId",
];

pub fn create_comment(ctx: &mut Ctx, args: &Value) -> R {
    let targets: Vec<&str> = TARGETS
        .iter()
        .copied()
        .filter(|t| arg(args, t).is_some())
        .collect();
    let parent_id = arg(args, "parentId").map(str::to_owned);
    if parent_id.is_none() && targets.is_empty() {
        return Err("A comment target ID or parentId is required".into());
    }
    if targets.len() > 1 {
        return Err("Only one comment target ID can be provided".into());
    }
    let body = s(args, "body").to_owned();
    let mut doc = json!({"body": body});
    let mut target_key = String::new();
    if let Some(p) = &parent_id {
        let parent = ctx.need("comments", p, "Comment")?;
        for t in TARGETS {
            if let Some(v) = opt(&parent, t) {
                doc[t] = v;
            }
        }
        doc["parentId"] = json!(s(&parent, "id"));
    }
    if let Some(t) = targets.first() {
        let v = arg(args, t).unwrap_or("");
        match *t {
            "issueId" => {
                let (_, i) = ctx.need_issue(v)?;
                doc["issueId"] = json!(s(&i, "id"));
            }
            "projectId" => {
                ctx.need("projects", v, "Project")?;
                doc["projectId"] = json!(v);
            }
            "initiativeId" => {
                ctx.need("initiatives", v, "Initiative")?;
                doc["initiativeId"] = json!(v);
            }
            "projectUpdateId" => {
                ctx.need("project_updates", v, "ProjectUpdate")?;
                doc["projectUpdateId"] = json!(v);
            }
            "initiativeUpdateId" => {
                ctx.need("initiative_updates", v, "InitiativeUpdate")?;
                doc["initiativeUpdateId"] = json!(v);
            }
            "documentContentId" => {
                if !ctx
                    .all("documents")
                    .iter()
                    .any(|d| s(d, "documentContentId") == v)
                {
                    return Err(not_found("DocumentContent"));
                }
                doc["documentContentId"] = json!(v);
            }
            _ => return Err(not_found("Post")),
        }
    }
    if let Some(issue_id) = doc
        .get("issueId")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        let (_, i) = ctx.need_issue(&issue_id)?;
        target_key = s(&i, "identifier").to_owned();
    } else {
        for t in TARGETS {
            if let Some(v) = doc.get(t).and_then(Value::as_str) {
                target_key = v.to_owned();
                break;
            }
        }
    }
    let n = ctx
        .entries("comments")
        .iter()
        .filter(|(k, _)| k.starts_with(&format!("{target_key}#")))
        .count()
        + 1;
    let id = ctx.uuid("comment");
    let now = ctx.iso_now();
    doc["id"] = json!(id);
    doc["userId"] = json!(ctx.viewer_id());
    doc["createdAt"] = json!(now);
    doc["updatedAt"] = json!(now);
    if let Some(q) = arg(args, "quotedText") {
        doc["quotedText"] = json!(q);
    }
    ctx.put("comments", &format!("{target_key}#{n}"), doc.clone());
    Ok(comment_out(ctx, &doc))
}

pub fn update_comment(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let mut c = ctx.need("comments", id, "Comment")?;
    let fields = [
        "body",
        "quotedText",
        "resolvingCommentId",
        "resolvingUserId",
    ];
    let any = fields.iter().any(|f| arg(args, f).is_some())
        || arg_strs(args, "subscriberIds").is_some_and(|v| !v.is_empty())
        || args
            .get("doNotSubscribeToIssue")
            .is_some_and(Value::is_boolean);
    if !any {
        return Err("At least one comment field must be provided".into());
    }
    if s(&c, "userId") != ctx.viewer_id() && arg(args, "body").is_some() {
        return Err("Forbidden - You can only edit your own comments.".into());
    }
    let now = ctx.iso_now();
    if let Some(b) = arg(args, "body") {
        c["body"] = json!(b);
        c["editedAt"] = json!(now);
    }
    if let Some(q) = arg(args, "quotedText") {
        c["quotedText"] = json!(q);
    }
    if let Some(r) = arg(args, "resolvingCommentId") {
        c["resolvingCommentId"] = json!(r);
        c["resolvedAt"] = json!(now);
    }
    if arg(args, "resolvingUserId").is_some() {
        c["resolvedAt"] = json!(now);
    }
    c["updatedAt"] = json!(now);
    ctx.save("comments", c.clone());
    Ok(comment_out(ctx, &c))
}

pub fn delete_comment(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let c = ctx.need("comments", id, "Comment")?;
    ctx.remove("comments", id);
    Ok(json!({"success": true, "id": s(&c, "id")}))
}

pub fn get_comments(ctx: &mut Ctx, args: &Value) -> R {
    let targets: Vec<&str> = TARGETS
        .iter()
        .copied()
        .filter(|t| arg(args, t).is_some())
        .collect();
    if targets.is_empty() {
        return Err("A comment target ID is required".into());
    }
    if targets.len() > 1 {
        return Err("Only one comment target ID can be provided".into());
    }
    let t = targets[0];
    let mut v = arg(args, t).unwrap_or("").to_owned();
    if t == "issueId" {
        let (_, i) = ctx.need_issue(&v)?;
        v = s(&i, "id").to_owned();
    }
    let mut list: Vec<Value> = ctx
        .all("comments")
        .into_iter()
        .filter(|c| s(c, t) == v && opt(c, "archivedAt").is_none())
        .collect();
    if arg(args, "orderBy") == Some("updatedAt") {
        list.sort_by(|a, b| s(b, "updatedAt").cmp(s(a, "updatedAt")));
    } else {
        list.sort_by(|a, b| s(b, "createdAt").cmp(s(a, "createdAt")));
    }
    if let Some(cursor) = arg(args, "cursor") {
        match list.iter().position(|c| s(c, "id") == cursor) {
            Some(p) => list = list.split_off(p + 1),
            None => return Err("Argument Validation Error - after must be a valid cursor.".into()),
        }
    }
    let limit = arg_i64(args, "limit").unwrap_or(25).max(0) as usize;
    Ok(Value::Array(
        list.iter()
            .take(limit)
            .map(|c| comment_out(ctx, c))
            .collect(),
    ))
}

// ───────────────────────────── attachments ─────────────────────────────

fn attachment_out(ctx: &Ctx, a: &Value) -> Value {
    obj(vec![
        ("id", json!(s(a, "id"))),
        ("title", json!(s(a, "title"))),
        ("subtitle", get_or_null(a, "subtitle")),
        ("url", json!(s(a, "url"))),
        ("metadata", a.get("metadata").cloned().unwrap_or(json!({}))),
        ("sourceType", get_or_null(a, "sourceType")),
        ("createdAt", json!(s(a, "createdAt"))),
        ("updatedAt", json!(s(a, "updatedAt"))),
        ("issue", ctx.issue_ref(s(a, "issueId"))),
    ])
}

pub fn get_attachments(ctx: &mut Ctx, args: &Value) -> R {
    let (_, i) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let limit = arg_i64(args, "limit").unwrap_or(25).max(0) as usize;
    let list: Vec<Value> = order(
        args,
        ctx.all("attachments")
            .into_iter()
            .filter(|a| s(a, "issueId") == s(&i, "id"))
            .filter(|a| {
                arg_bool(args, "includeArchived").unwrap_or(false) || opt(a, "archivedAt").is_none()
            })
            .collect(),
    );
    Ok(Value::Array(
        list.iter()
            .take(limit)
            .map(|a| attachment_out(ctx, a))
            .collect(),
    ))
}

pub fn add_attachment(ctx: &mut Ctx, args: &Value) -> R {
    let (_, i) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let url = arg(args, "url")
        .ok_or_else(|| "Argument Validation Error - url must be a URL address.".to_owned())?;
    let existing = ctx
        .all("attachments")
        .into_iter()
        .find(|a| s(a, "issueId") == s(&i, "id") && s(a, "url") == url);
    let now = ctx.iso_now();
    let mut doc = match existing {
        Some(a) => a,
        None => {
            let id = ctx.uuid("attachment");
            json!({"id": id, "issueId": s(&i, "id"), "createdAt": now, "sourceType": null})
        }
    };
    doc["title"] = json!(s(args, "title"));
    doc["url"] = json!(url);
    doc["subtitle"] = arg(args, "subtitle")
        .map(|x| json!(x))
        .unwrap_or(Value::Null);
    doc["metadata"] = args
        .get("metadata")
        .filter(|m| m.as_object().is_some_and(|o| !o.is_empty()))
        .cloned()
        .unwrap_or(json!({}));
    doc["updatedAt"] = json!(now);
    doc["creatorId"] = json!(ctx.viewer_id());
    ctx.save("attachments", doc.clone());
    if let Some(body) = arg(args, "commentBody") {
        create_comment(ctx, &json!({"issueId": s(&i, "id"), "body": body}))?;
    }
    Ok(attachment_out(ctx, &doc))
}

// ───────────────────────────── templates ─────────────────────────────

pub fn template_out(ctx: &Ctx, t: &Value) -> Value {
    obj(vec![
        ("id", json!(s(t, "id"))),
        ("name", json!(s(t, "name"))),
        ("description", get_or_null(t, "description")),
        ("sortOrder", t.get("sortOrder").cloned().unwrap_or(json!(0))),
        (
            "type",
            json!(match s(t, "type") {
                "" => "issue",
                x => x,
            }),
        ),
        ("archivedAt", get_or_null(t, "archivedAt")),
        ("createdAt", json!(s(t, "createdAt"))),
        ("updatedAt", json!(s(t, "updatedAt"))),
        (
            "templateData",
            t.get("templateData").cloned().unwrap_or(json!({})),
        ),
        ("team", ctx.team_ref_key(s(t, "teamId"))),
        ("creator", ctx.user_ref_email(s(t, "creatorId"))),
    ])
}

fn issue_templates(ctx: &Ctx, args: &Value, team: Option<&str>) -> Vec<Value> {
    let include_archived = arg_bool(args, "includeArchived").unwrap_or(false);
    let list: Vec<Value> = ctx
        .all("templates")
        .into_iter()
        .filter(|t| matches!(s(t, "type"), "" | "issue"))
        .filter(|t| include_archived || opt(t, "archivedAt").is_none())
        .filter(|t| team.is_none_or(|x| s(t, "teamId") == x))
        .collect();
    let limit = arg_i64(args, "limit").unwrap_or(25).max(0) as usize;
    order(args, list).into_iter().take(limit).collect()
}

pub fn get_templates(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        issue_templates(ctx, args, None)
            .iter()
            .map(|t| template_out(ctx, t))
            .collect(),
    ))
}

pub fn get_team_templates(ctx: &mut Ctx, args: &Value) -> R {
    let team = ctx.need("teams", arg(args, "teamId").unwrap_or(""), "Team")?;
    Ok(Value::Array(
        issue_templates(ctx, args, Some(s(&team, "id")))
            .iter()
            .map(|t| template_out(ctx, t))
            .collect(),
    ))
}

pub fn get_template(ctx: &mut Ctx, args: &Value) -> R {
    let t = ctx.need("templates", arg(args, "id").unwrap_or(""), "Template")?;
    Ok(template_out(ctx, &t))
}

pub fn create_template(ctx: &mut Ctx, args: &Value) -> R {
    if let Some(team) = arg(args, "teamId") {
        ctx.need("teams", team, "Team")?;
    }
    let id = ctx.uuid("template");
    let now = ctx.iso_now();
    let doc = json!({
        "id": id, "name": s(args, "name"), "description": arg(args, "description"),
        "teamId": arg(args, "teamId"), "templateData": args.get("templateData").cloned().unwrap_or(json!({})),
        "sortOrder": args.get("sortOrder").cloned().unwrap_or(json!(0)), "type": "issue",
        "createdAt": now, "updatedAt": now, "creatorId": ctx.viewer_id(), "archivedAt": null,
    });
    ctx.put("templates", &id, doc.clone());
    Ok(template_out(ctx, &doc))
}

pub fn update_template(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let mut t = ctx.need("templates", id, "Template")?;
    let mut any = false;
    if let Some(n) = arg(args, "name") {
        t["name"] = json!(n);
        any = true;
    }
    for f in ["description", "teamId"] {
        match args.get(f) {
            Some(Value::Null) => {
                t[f] = Value::Null;
                any = true;
            }
            Some(Value::String(x)) if !x.trim().is_empty() => {
                t[f] = json!(x);
                any = true;
            }
            _ => {}
        }
    }
    if let Some(d) = args
        .get("templateData")
        .filter(|d| d.as_object().is_some_and(|o| !o.is_empty()))
    {
        t["templateData"] = d.clone();
        any = true;
    }
    if let Some(so) = args.get("sortOrder").filter(|x| !x.is_null()) {
        t["sortOrder"] = so.clone();
        any = true;
    }
    if !any {
        return Err("At least one template field must be provided".into());
    }
    t["updatedAt"] = json!(ctx.iso_now());
    ctx.save("templates", t.clone());
    Ok(template_out(ctx, &t))
}

pub fn archive_template(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let mut t = ctx.need("templates", id, "Template")?;
    t["archivedAt"] = json!(ctx.iso_now());
    ctx.save("templates", t.clone());
    Ok(json!({"success": true, "id": s(&t, "id")}))
}

pub fn create_from_template(ctx: &mut Ctx, args: &Value) -> R {
    let mut a = json!({
        "teamId": s(args, "teamId"),
        "templateId": s(args, "templateId"),
        "title": arg(args, "title").unwrap_or("Template issue"),
    });
    for f in [
        "description",
        "priority",
        "projectId",
        "projectMilestoneId",
        "cycleId",
    ] {
        if let Some(v) = args.get(f).filter(|v| !v.is_null()) {
            a[f] = v.clone();
        }
    }
    create_issue(ctx, &a)
}

// ───────────────────────────── custom fields ─────────────────────────────

pub fn get_custom_fields(_ctx: &mut Ctx, _args: &Value) -> R {
    Err("The authenticated Linear schema does not expose a custom field definitions query".into())
}

pub fn get_issue_custom_fields(ctx: &mut Ctx, args: &Value) -> R {
    ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    Err("The authenticated Linear schema does not expose issue custom field values on Issue".into())
}

pub fn update_issue_custom_field(_ctx: &mut Ctx, _args: &Value) -> R {
    Err(
        "The authenticated Linear schema does not expose an issue custom field update mutation"
            .into(),
    )
}

/// Issues of a cycle, project, ... as raw summary nodes, newest first.
pub fn summaries(ctx: &Ctx, args: &Value, keep: impl Fn(&Value) -> bool) -> Vec<Value> {
    let limit = arg_i64(args, "limit").unwrap_or(25).max(0) as usize;
    let list: Vec<Value> = visible_issues(ctx)
        .into_iter()
        .filter(|i| keep(i) && pm_filter(ctx, args, i))
        .collect();
    order(args, list)
        .iter()
        .take(limit)
        .map(|i| issue_summary(ctx, i))
        .collect()
}

pub fn completed(ctx: &Ctx, i: &Value) -> bool {
    opt(i, "completedAt").is_some() || state_type(ctx, i) == "completed"
}

pub fn is_iso(text: &str) -> bool {
    norm_time(text).is_some()
}

pub fn iso_of(secs: i64) -> String {
    iso(secs)
}
