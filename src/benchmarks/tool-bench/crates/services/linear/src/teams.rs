//! Viewer, organization, users, labels, teams, workflow states, memberships and cycles.

use serde_json::{Value, json};

use crate::{
    issues::{completed, summaries, visible_issues},
    store::{
        Ctx, R, arg, arg_bool, arg_i64, get_or_null, norm_time, not_found, obj, opt, parse_time, s,
        sdk,
    },
};

fn list_args(args: &Value) -> (usize, bool) {
    (
        arg_i64(args, "limit").unwrap_or(25).max(0) as usize,
        arg_bool(args, "includeArchived").unwrap_or(false),
    )
}

fn sort_by(args: &Value, mut v: Vec<Value>) -> Vec<Value> {
    match arg(args, "orderBy") {
        Some("updatedAt") => v.sort_by(|a, b| s(b, "updatedAt").cmp(s(a, "updatedAt"))),
        Some("createdAt") => v.sort_by(|a, b| s(b, "createdAt").cmp(s(a, "createdAt"))),
        _ => {}
    }
    v
}

fn user_info(u: &Value) -> Value {
    sdk(vec![
        ("id", json!(s(u, "id"))),
        ("name", json!(s(u, "name"))),
        ("email", json!(s(u, "email"))),
        ("displayName", json!(s(u, "displayName"))),
        (
            "active",
            json!(u.get("active").and_then(Value::as_bool).unwrap_or(true)),
        ),
    ])
}

pub fn get_viewer(ctx: &mut Ctx, _args: &Value) -> R {
    let u = ctx.need("users", &ctx.viewer_id(), "User")?;
    Ok(user_info(&u))
}

pub fn get_organization(ctx: &mut Ctx, _args: &Value) -> R {
    let o = ctx.org();
    let mut out = sdk(vec![
        ("id", json!(s(&o, "id"))),
        ("name", json!(s(&o, "name"))),
        ("urlKey", json!(ctx.url_key())),
        ("logoUrl", get_or_null(&o, "logoUrl")),
        ("createdAt", get_or_null(&o, "createdAt")),
    ]);
    out["subscription"] = get_or_null(&o, "subscription");
    Ok(out)
}

pub fn get_users(ctx: &mut Ctx, _args: &Value) -> R {
    Ok(Value::Array(
        ctx.all("users").iter().map(user_info).collect(),
    ))
}

pub fn get_labels(ctx: &mut Ctx, _args: &Value) -> R {
    Ok(Value::Array(
        ctx.all("labels")
            .iter()
            .filter(|l| opt(l, "archivedAt").is_none())
            .map(|l| {
                let mut o = sdk(vec![
                    ("id", json!(s(l, "id"))),
                    ("name", json!(s(l, "name"))),
                    ("color", get_or_null(l, "color")),
                    ("description", get_or_null(l, "description")),
                ]);
                o["team"] = ctx.team_ref(s(l, "teamId"));
                o
            })
            .collect(),
    ))
}

pub fn get_teams(ctx: &mut Ctx, _args: &Value) -> R {
    Ok(Value::Array(
        ctx.all("teams")
            .iter()
            .filter(|t| opt(t, "archivedAt").is_none())
            .map(|t| {
                sdk(vec![
                    ("id", json!(s(t, "id"))),
                    ("name", json!(s(t, "name"))),
                    ("key", json!(s(t, "key"))),
                    ("description", get_or_null(t, "description")),
                ])
            })
            .collect(),
    ))
}

pub fn team_out(ctx: &Ctx, t: &Value) -> Value {
    let private = t.get("private").and_then(Value::as_bool).unwrap_or(false);
    let mut o = sdk(vec![
        ("id", json!(s(t, "id"))),
        ("name", json!(s(t, "name"))),
        ("key", json!(s(t, "key"))),
        ("description", get_or_null(t, "description")),
    ]);
    let m = o.as_object_mut().expect("object");
    m.insert("color".into(), get_or_null(t, "color"));
    m.insert("icon".into(), get_or_null(t, "icon"));
    m.insert("private".into(), json!(private));
    m.insert(
        "visibility".into(),
        json!(match s(t, "visibility") {
            "" =>
                if private {
                    "private"
                } else {
                    "public"
                },
            v => v,
        }),
    );
    m.insert("parent".into(), ctx.team_ref_key(s(t, "parentId")));
    m.insert("timezone".into(), get_or_null(t, "timezone"));
    m.insert("archivedAt".into(), get_or_null(t, "archivedAt"));
    o
}

fn team_fields(ctx: &Ctx, args: &Value, t: &mut Value) -> Result<bool, String> {
    let mut any = false;
    for f in [
        "name",
        "key",
        "description",
        "color",
        "icon",
        "timezone",
        "visibility",
    ] {
        if let Some(v) = arg(args, f) {
            if f == "key" {
                let taken = ctx
                    .all("teams")
                    .iter()
                    .any(|x| s(x, "key").eq_ignore_ascii_case(v) && s(x, "id") != s(t, "id"));
                if taken {
                    return Err("Invalid input - Team key already in use.".into());
                }
                t[f] = json!(v.to_uppercase());
            } else {
                t[f] = json!(v);
            }
            any = true;
        }
    }
    if let Some(p) = arg(args, "parentId") {
        ctx.need("teams", p, "Team")?;
        t["parentId"] = json!(p);
        any = true;
    }
    if let Some(p) = arg_bool(args, "private") {
        t["private"] = json!(p);
        any = true;
    }
    Ok(any)
}

pub fn create_team(ctx: &mut Ctx, args: &Value) -> R {
    let name = s(args, "name").to_owned();
    let id = ctx.uuid("team");
    let key = arg(args, "key").map(str::to_uppercase).unwrap_or_else(|| {
        name.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(3)
            .collect::<String>()
            .to_uppercase()
    });
    let now = ctx.iso_now();
    let mut t = json!({"id": id, "name": name, "key": key, "createdAt": now, "updatedAt": now, "private": false, "timezone": "America/Los_Angeles"});
    team_fields(ctx, args, &mut t)?;
    ctx.put("teams", &id, t.clone());
    // Linear creates the default workflow for a new team.
    for (pos, (n, ty, color)) in [
        ("Backlog", "backlog", "#bec2c8"),
        ("Todo", "unstarted", "#e2e2e2"),
        ("In Progress", "started", "#f2c94c"),
        ("Done", "completed", "#5e6ad2"),
        ("Canceled", "canceled", "#95a2b3"),
        ("Duplicate", "canceled", "#95a2b3"),
    ]
    .iter()
    .enumerate()
    {
        let sid = ctx.uuid("workflow_state");
        ctx.put(
            "workflow_states",
            &sid,
            json!({"id": sid, "teamId": id, "name": n, "type": ty, "color": color, "position": pos as i64, "description": null, "createdAt": now}),
        );
    }
    let mid = ctx.uuid("membership");
    let viewer = ctx.viewer_id();
    ctx.put(
        "team_memberships",
        &mid,
        json!({"id": mid, "teamId": id, "userId": viewer, "owner": true, "sortOrder": 0, "createdAt": now, "updatedAt": now}),
    );
    Ok(team_out(ctx, &t))
}

pub fn update_team(ctx: &mut Ctx, args: &Value) -> R {
    let mut t = ctx.need("teams", arg(args, "id").unwrap_or(""), "Team")?;
    if !team_fields(ctx, args, &mut t)? {
        return Err("At least one team field must be provided".into());
    }
    t["updatedAt"] = json!(ctx.iso_now());
    ctx.save("teams", t.clone());
    Ok(team_out(ctx, &t))
}

pub fn archive_team(ctx: &mut Ctx, args: &Value) -> R {
    let mut t = ctx.need("teams", arg(args, "id").unwrap_or(""), "Team")?;
    t["archivedAt"] = json!(ctx.iso_now());
    ctx.save("teams", t.clone());
    Ok(json!({"success": true, "id": s(&t, "id")}))
}

pub fn get_workflow_states(ctx: &mut Ctx, args: &Value) -> R {
    let team = arg(args, "teamId").unwrap_or("");
    let include_archived = arg_bool(args, "includeArchived").unwrap_or(false);
    let mut states: Vec<Value> = ctx
        .all("workflow_states")
        .into_iter()
        .filter(|st| {
            s(st, "teamId") == team && (include_archived || opt(st, "archivedAt").is_none())
        })
        .collect();
    states.sort_by(|a, b| {
        a["position"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&b["position"].as_f64().unwrap_or(0.0))
    });
    Ok(Value::Array(
        states
            .iter()
            .map(|st| {
                json!({
                    "id": s(st, "id"), "name": s(st, "name"), "type": s(st, "type"),
                    "position": st.get("position").cloned().unwrap_or(json!(0)),
                    "color": s(st, "color"), "description": s(st, "description"),
                })
            })
            .collect(),
    ))
}

fn state_out(ctx: &Ctx, st: &Value) -> Value {
    obj(vec![
        ("id", json!(s(st, "id"))),
        ("name", json!(s(st, "name"))),
        ("type", json!(s(st, "type"))),
        ("position", st.get("position").cloned().unwrap_or(json!(0))),
        ("color", json!(s(st, "color"))),
        ("description", get_or_null(st, "description")),
        ("team", ctx.team_ref_key(s(st, "teamId"))),
    ])
}

pub fn create_workflow_state(ctx: &mut Ctx, args: &Value) -> R {
    let team = ctx.need("teams", arg(args, "teamId").unwrap_or(""), "Team")?;
    let ty = s(args, "type");
    if ![
        "triage",
        "backlog",
        "unstarted",
        "started",
        "completed",
        "canceled",
    ]
    .contains(&ty)
    {
        return Err(format!(
            "Argument Validation Error - type must be one of: triage, backlog, unstarted, started, completed, canceled (got \"{ty}\")."
        ));
    }
    let id = ctx.uuid("workflow_state");
    let position = args.get("position").cloned().unwrap_or_else(|| {
        json!(
            ctx.all("workflow_states")
                .iter()
                .filter(|x| s(x, "teamId") == s(&team, "id"))
                .count()
        )
    });
    let doc = json!({
        "id": id, "teamId": s(&team, "id"), "name": s(args, "name"), "type": ty, "color": s(args, "color"),
        "position": position, "description": arg(args, "description"), "createdAt": ctx.iso_now(),
    });
    ctx.put("workflow_states", &id, doc.clone());
    Ok(state_out(ctx, &doc))
}

pub fn update_workflow_state(ctx: &mut Ctx, args: &Value) -> R {
    let mut st = ctx.need(
        "workflow_states",
        arg(args, "id").unwrap_or(""),
        "WorkflowState",
    )?;
    let mut any = false;
    for f in ["name", "color", "description"] {
        if let Some(v) = arg(args, f) {
            st[f] = json!(v);
            any = true;
        }
    }
    if let Some(p) = args.get("position").filter(|p| p.is_number()) {
        st["position"] = p.clone();
        any = true;
    }
    if !any {
        return Err("At least one workflow state field must be provided".into());
    }
    ctx.save("workflow_states", st.clone());
    Ok(state_out(ctx, &st))
}

fn membership_out(ctx: &Ctx, m: &Value) -> Value {
    obj(vec![
        ("id", json!(s(m, "id"))),
        (
            "owner",
            json!(m.get("owner").and_then(Value::as_bool).unwrap_or(false)),
        ),
        ("sortOrder", get_or_null(m, "sortOrder")),
        ("createdAt", json!(s(m, "createdAt"))),
        ("updatedAt", json!(s(m, "updatedAt"))),
        ("team", ctx.team_ref_key(s(m, "teamId"))),
        ("user", ctx.simple_user(s(m, "userId"))),
    ])
}

pub fn get_team_memberships(ctx: &mut Ctx, args: &Value) -> R {
    let team = ctx.need("teams", arg(args, "teamId").unwrap_or(""), "Team")?;
    let (limit, archived) = list_args(args);
    let list: Vec<Value> = sort_by(
        args,
        ctx.all("team_memberships")
            .into_iter()
            .filter(|m| {
                s(m, "teamId") == s(&team, "id") && (archived || opt(m, "archivedAt").is_none())
            })
            .collect(),
    );
    Ok(Value::Array(
        list.iter()
            .take(limit)
            .map(|m| membership_out(ctx, m))
            .collect(),
    ))
}

pub fn add_user_to_team(ctx: &mut Ctx, args: &Value) -> R {
    let team = ctx.need("teams", arg(args, "teamId").unwrap_or(""), "Team")?;
    let user = ctx.need("users", arg(args, "userId").unwrap_or(""), "User")?;
    if ctx.all("team_memberships").iter().any(|m| {
        s(m, "teamId") == s(&team, "id")
            && s(m, "userId") == s(&user, "id")
            && opt(m, "archivedAt").is_none()
    }) {
        return Err("Invalid input - User is already a member of this team.".into());
    }
    let id = ctx.uuid("membership");
    let now = ctx.iso_now();
    let doc = json!({
        "id": id, "teamId": s(&team, "id"), "userId": s(&user, "id"),
        "owner": arg_bool(args, "owner").unwrap_or(false),
        "sortOrder": args.get("sortOrder").cloned().unwrap_or(Value::Null),
        "createdAt": now, "updatedAt": now,
    });
    ctx.put("team_memberships", &id, doc.clone());
    Ok(membership_out(ctx, &doc))
}

pub fn remove_user_from_team(ctx: &mut Ctx, args: &Value) -> R {
    let team_id = arg(args, "teamId").unwrap_or("").to_owned();
    let user_id = arg(args, "userId").unwrap_or("").to_owned();
    ctx.need("teams", &team_id, "Team")?;
    let m = ctx
        .all("team_memberships")
        .into_iter()
        .find(|m| s(m, "teamId") == team_id && s(m, "userId") == user_id)
        .ok_or_else(|| format!("User {user_id} is not a member of team {team_id}"))?;
    ctx.remove("team_memberships", s(&m, "id"));
    Ok(json!({"success": true, "teamId": team_id, "userId": user_id}))
}

pub fn update_team_membership(ctx: &mut Ctx, args: &Value) -> R {
    let mut m = ctx.need(
        "team_memberships",
        arg(args, "id").unwrap_or(""),
        "TeamMembership",
    )?;
    let mut any = false;
    if let Some(o) = arg_bool(args, "owner") {
        m["owner"] = json!(o);
        any = true;
    }
    if let Some(so) = args.get("sortOrder").filter(|x| !x.is_null()) {
        m["sortOrder"] = so.clone();
        any = true;
    }
    if !any {
        return Err("At least one team membership field must be provided".into());
    }
    m["updatedAt"] = json!(ctx.iso_now());
    ctx.save("team_memberships", m.clone());
    Ok(membership_out(ctx, &m))
}

pub fn label_out(ctx: &Ctx, l: &Value) -> Value {
    let parent = match ctx.get("labels", s(l, "parentId")) {
        Some(p) => json!({"id": s(&p, "id"), "name": s(&p, "name")}),
        None => Value::Null,
    };
    obj(vec![
        ("id", json!(s(l, "id"))),
        ("name", json!(s(l, "name"))),
        ("color", json!(s(l, "color"))),
        ("description", get_or_null(l, "description")),
        ("team", ctx.team_ref_key(s(l, "teamId"))),
        ("parent", parent),
    ])
}

pub fn get_team_labels(ctx: &mut Ctx, args: &Value) -> R {
    let team = ctx.need("teams", arg(args, "teamId").unwrap_or(""), "Team")?;
    let (limit, archived) = list_args(args);
    let list: Vec<Value> = sort_by(
        args,
        ctx.all("labels")
            .into_iter()
            .filter(|l| {
                s(l, "teamId") == s(&team, "id") && (archived || opt(l, "archivedAt").is_none())
            })
            .collect(),
    );
    Ok(Value::Array(
        list.iter().take(limit).map(|l| label_out(ctx, l)).collect(),
    ))
}

pub fn create_team_label(ctx: &mut Ctx, args: &Value) -> R {
    let team = ctx.need("teams", arg(args, "teamId").unwrap_or(""), "Team")?;
    let name = s(args, "name").to_owned();
    if ctx.all("labels").iter().any(|l| {
        s(l, "name").eq_ignore_ascii_case(&name)
            && (s(l, "teamId") == s(&team, "id") || s(l, "teamId").is_empty())
    }) {
        return Err("Invalid input - Duplicate label name.".into());
    }
    if let Some(p) = arg(args, "parentId") {
        ctx.need("labels", p, "IssueLabel")?;
    }
    let id = ctx.uuid("label");
    let now = ctx.iso_now();
    let doc = json!({
        "id": id, "name": name, "teamId": s(&team, "id"),
        "color": arg(args, "color").unwrap_or("#bec2c8"), "description": arg(args, "description"),
        "parentId": arg(args, "parentId"), "createdAt": now, "updatedAt": now,
    });
    ctx.put("labels", &id, doc.clone());
    Ok(label_out(ctx, &doc))
}

// ───────────────────────────── cycles ─────────────────────────────

fn cycle_issues(ctx: &Ctx, cycle_id: &str) -> Vec<Value> {
    visible_issues(ctx)
        .into_iter()
        .filter(|i| s(i, "cycleId") == cycle_id)
        .collect()
}

fn progress(ctx: &Ctx, cycle_id: &str) -> f64 {
    let issues = cycle_issues(ctx, cycle_id);
    if issues.is_empty() {
        return 0.0;
    }
    let done = issues.iter().filter(|i| completed(ctx, i)).count();
    (done as f64 / issues.len() as f64 * 100.0).round() / 100.0
}

pub fn cycle_out(ctx: &Ctx, c: &Value) -> Value {
    let mut o = sdk(vec![
        ("id", json!(s(c, "id"))),
        ("number", c.get("number").cloned().unwrap_or(json!(0))),
        ("name", get_or_null(c, "name")),
        ("description", get_or_null(c, "description")),
        ("startsAt", json!(s(c, "startsAt"))),
        ("endsAt", json!(s(c, "endsAt"))),
    ]);
    o["completedAt"] = get_or_null(c, "completedAt");
    o["progress"] = json!(progress(ctx, s(c, "id")));
    o["team"] = ctx.team_ref_key(s(c, "teamId"));
    o
}

pub fn get_cycles(ctx: &mut Ctx, args: &Value) -> R {
    let limit = arg_i64(args, "limit").unwrap_or(25).max(0) as usize;
    let team = arg(args, "teamId");
    let list: Vec<Value> = ctx
        .all("cycles")
        .into_iter()
        .filter(|c| team.is_none_or(|t| s(c, "teamId") == t) && opt(c, "archivedAt").is_none())
        .take(limit)
        .map(|c| {
            let mut o = sdk(vec![
                ("id", json!(s(&c, "id"))),
                ("number", c.get("number").cloned().unwrap_or(json!(0))),
                ("name", get_or_null(&c, "name")),
                ("description", get_or_null(&c, "description")),
                ("startsAt", json!(s(&c, "startsAt"))),
                ("endsAt", json!(s(&c, "endsAt"))),
                ("completedAt", get_or_null(&c, "completedAt")),
            ]);
            o["team"] = ctx.team_ref_key(s(&c, "teamId"));
            o
        })
        .collect();
    Ok(Value::Array(list))
}

pub fn get_cycle(ctx: &mut Ctx, args: &Value) -> R {
    let c = ctx.need("cycles", arg(args, "id").unwrap_or(""), "Cycle")?;
    Ok(cycle_out(ctx, &c))
}

pub fn create_cycle(ctx: &mut Ctx, args: &Value) -> R {
    let team = ctx.need("teams", arg(args, "teamId").unwrap_or(""), "Team")?;
    let starts = norm_time(s(args, "startsAt")).ok_or_else(|| {
        "Argument Validation Error - startsAt must be a Date instance.".to_owned()
    })?;
    let ends = norm_time(s(args, "endsAt"))
        .ok_or_else(|| "Argument Validation Error - endsAt must be a Date instance.".to_owned())?;
    if ends <= starts {
        return Err("Invalid input - Cycle end date must be after the start date.".into());
    }
    let number = ctx
        .all("cycles")
        .iter()
        .filter(|c| s(c, "teamId") == s(&team, "id"))
        .filter_map(|c| c["number"].as_i64())
        .max()
        .unwrap_or(0)
        + 1;
    let id = ctx.uuid("cycle");
    let now = ctx.iso_now();
    let doc = json!({
        "id": id, "teamId": s(&team, "id"), "number": number, "name": arg(args, "name"),
        "description": arg(args, "description"), "startsAt": starts, "endsAt": ends,
        "completedAt": arg(args, "completedAt").and_then(norm_time), "createdAt": now, "updatedAt": now,
    });
    ctx.put("cycles", &id, doc.clone());
    Ok(cycle_out(ctx, &doc))
}

pub fn update_cycle(ctx: &mut Ctx, args: &Value) -> R {
    let mut c = ctx.need("cycles", arg(args, "id").unwrap_or(""), "Cycle")?;
    let mut any = false;
    for f in ["startsAt", "endsAt", "completedAt"] {
        if let Some(v) = arg(args, f) {
            c[f] = json!(norm_time(v).ok_or_else(|| format!(
                "Argument Validation Error - {f} must be a Date instance."
            ))?);
            any = true;
        }
    }
    for f in ["name", "description"] {
        if let Some(v) = arg(args, f) {
            c[f] = json!(v);
            any = true;
        }
    }
    if !any {
        return Err("At least one cycle field must be provided".into());
    }
    c["updatedAt"] = json!(ctx.iso_now());
    ctx.save("cycles", c.clone());
    Ok(cycle_out(ctx, &c))
}

pub fn complete_cycle(ctx: &mut Ctx, args: &Value) -> R {
    let now = ctx.iso_now();
    update_cycle(ctx, &json!({"id": s(args, "id"), "completedAt": now}))
}

pub fn cycle_stats(ctx: &mut Ctx, args: &Value) -> R {
    let c = ctx.need("cycles", arg(args, "id").unwrap_or(""), "Cycle")?;
    let issues = cycle_issues(ctx, s(&c, "id"));
    let total = issues.len().min(100);
    let done = issues
        .iter()
        .take(100)
        .filter(|i| opt(i, "completedAt").is_some())
        .count();
    let pct = if total > 0 {
        (done as f64 / total as f64 * 10_000.0).round() / 100.0
    } else {
        0.0
    };
    let mut o = sdk(vec![
        ("id", json!(s(&c, "id"))),
        ("number", c.get("number").cloned().unwrap_or(json!(0))),
        ("name", get_or_null(&c, "name")),
    ]);
    let m = o.as_object_mut().expect("object");
    m.insert("progress".into(), json!(pct));
    m.insert("issueCount".into(), json!(total));
    m.insert("completedIssueCount".into(), json!(done));
    for h in [
        "scopeHistory",
        "completedScopeHistory",
        "completedIssueCountHistory",
        "issueCountHistory",
    ] {
        m.insert(h.into(), c.get(h).cloned().unwrap_or(json!([])));
    }
    Ok(o)
}

pub fn active_cycle_of(ctx: &Ctx, team_id: &str) -> Option<Value> {
    ctx.all("cycles").into_iter().find(|c| {
        s(c, "teamId") == team_id
            && opt(c, "completedAt").is_none()
            && parse_time(s(c, "startsAt")).is_some_and(|t| t <= ctx.now)
            && parse_time(s(c, "endsAt")).is_some_and(|t| t > ctx.now)
    })
}

pub fn get_active_cycle(ctx: &mut Ctx, args: &Value) -> R {
    let team = ctx.need("teams", arg(args, "teamId").unwrap_or(""), "Team")?;
    let c = active_cycle_of(ctx, s(&team, "id"))
        .ok_or_else(|| format!("No active cycle found for team {}", s(&team, "name")))?;
    let issues = cycle_issues(ctx, s(&c, "id"));
    let total = issues.len();
    let done = issues
        .iter()
        .filter(|i| opt(i, "completedAt").is_some())
        .count();
    let pct = if total > 0 {
        done as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    let mut o = sdk(vec![
        ("id", json!(s(&c, "id"))),
        ("number", c.get("number").cloned().unwrap_or(json!(0))),
        ("name", get_or_null(&c, "name")),
        ("description", get_or_null(&c, "description")),
        ("startsAt", json!(s(&c, "startsAt"))),
        ("endsAt", json!(s(&c, "endsAt"))),
    ]);
    let m = o.as_object_mut().expect("object");
    m.insert(
        "team".into(),
        json!({"id": s(&team, "id"), "name": s(&team, "name"), "key": s(&team, "key")}),
    );
    m.insert("progress".into(), json!((pct * 100.0).round() / 100.0));
    m.insert("issueCount".into(), json!(total));
    m.insert("completedIssueCount".into(), json!(done));
    Ok(o)
}

pub fn get_cycle_issues(ctx: &mut Ctx, args: &Value) -> R {
    let cycle = arg(args, "cycleId").unwrap_or("").to_owned();
    let mut a = args.clone();
    if let Some(m) = a.as_object_mut() {
        m.remove("cycleId");
    }
    Ok(Value::Array(summaries(ctx, &a, |i| {
        s(i, "cycleId") == cycle
    })))
}

pub fn add_issue_to_cycle(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let cycle = ctx.need("cycles", arg(args, "cycleId").unwrap_or(""), "Cycle")?;
    if s(&cycle, "teamId") != s(&issue, "teamId") {
        return Err("Invalid input - The cycle must belong to the issue's team.".into());
    }
    let issue_id = s(&issue, "id").to_owned();
    ctx.history(
        &issue_id,
        "cycle",
        get_or_null(&issue, "cycleId"),
        json!(s(&cycle, "id")),
    );
    issue["cycleId"] = json!(s(&cycle, "id"));
    issue["updatedAt"] = json!(ctx.iso_now());
    ctx.put("issues", &key, issue.clone());
    Ok(json!({"success": true, "issue": {
        "id": s(&issue, "id"), "identifier": s(&issue, "identifier"), "title": s(&issue, "title"),
        "cycle": sdk(vec![("id", json!(s(&cycle, "id"))), ("number", cycle["number"].clone()), ("name", get_or_null(&cycle, "name"))]),
    }}))
}

pub fn remove_issue_from_cycle(ctx: &mut Ctx, args: &Value) -> R {
    let (key, mut issue) = ctx.need_issue(arg(args, "issueId").unwrap_or(""))?;
    let cycle_id = arg(args, "cycleId").unwrap_or("").to_owned();
    if s(&issue, "cycleId") != cycle_id {
        return Err(format!(
            "Issue {} is not associated with cycle {cycle_id}",
            s(&issue, "identifier")
        ));
    }
    let cycle = ctx
        .get("cycles", &cycle_id)
        .ok_or_else(|| not_found("Cycle"))?;
    issue["cycleId"] = Value::Null;
    issue["updatedAt"] = json!(ctx.iso_now());
    ctx.put("issues", &key, issue.clone());
    Ok(json!({
        "success": true,
        "issue": {"id": s(&issue, "id"), "identifier": s(&issue, "identifier"), "title": s(&issue, "title")},
        "cycle": sdk(vec![("id", json!(s(&cycle, "id"))), ("number", cycle["number"].clone()), ("name", get_or_null(&cycle, "name"))]),
    }))
}
