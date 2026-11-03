//! Releases, customers, webhooks, notifications, sessions, audit, integrations, saved views,
//! favorites, OAuth applications and server tools.

use serde_json::{Value, json};

use crate::{
    issues::order,
    store::{Ctx, R, arg, arg_bool, arg_i64, arg_strs, get_or_null, not_found, obj, opt, s, sdk},
};

fn limit(args: &Value, default: i64) -> usize {
    arg_i64(args, "limit").unwrap_or(default).max(0) as usize
}

fn archived_ok(args: &Value, v: &Value) -> bool {
    arg_bool(args, "includeArchived").unwrap_or(false) || opt(v, "archivedAt").is_none()
}

fn listed(ctx: &Ctx, coll: &str, args: &Value, default: i64) -> Vec<Value> {
    order(
        args,
        ctx.all(coll)
            .into_iter()
            .filter(|v| archived_ok(args, v))
            .collect(),
    )
    .into_iter()
    .take(limit(args, default))
    .collect()
}

// ───────────────────────────── releases ─────────────────────────────

fn pipeline_ref(ctx: &Ctx, id: &str) -> Value {
    match ctx.get("release_pipelines", id) {
        Some(p) => {
            json!({"id": s(&p, "id"), "name": s(&p, "name"), "slugId": get_or_null(&p, "slugId"), "type": s(&p, "type")})
        }
        None => Value::Null,
    }
}

fn release_ref(ctx: &Ctx, id: &str) -> Value {
    match ctx.get("releases", id) {
        Some(r) => {
            json!({"id": s(&r, "id"), "name": s(&r, "name"), "version": get_or_null(&r, "version")})
        }
        None => json!({"id": id, "name": "", "version": null}),
    }
}

fn stage_out(ctx: &Ctx, st: &Value, with_pipeline: bool) -> Value {
    let mut o = obj(vec![
        ("id", json!(s(st, "id"))),
        ("name", json!(s(st, "name"))),
        ("color", get_or_null(st, "color")),
        ("type", json!(s(st, "type"))),
        ("position", get_or_null(st, "position")),
        ("description", get_or_null(st, "description")),
        ("frozen", json!(st["frozen"].as_bool().unwrap_or(false))),
        ("createdAt", get_or_null(st, "createdAt")),
        ("updatedAt", get_or_null(st, "updatedAt")),
        ("archivedAt", get_or_null(st, "archivedAt")),
    ]);
    if with_pipeline {
        o["pipeline"] = pipeline_ref(ctx, s(st, "pipelineId"));
    }
    o
}

fn note_out(ctx: &Ctx, n: &Value) -> Value {
    let releases: Vec<Value> = n["releaseIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| release_ref(ctx, r.as_str().unwrap_or("")))
                .collect()
        })
        .unwrap_or_default();
    let last = releases.last().cloned().unwrap_or(Value::Null);
    obj(vec![
        ("id", json!(s(n, "id"))),
        ("title", get_or_null(n, "title")),
        ("slugId", get_or_null(n, "slugId")),
        ("content", get_or_null(n, "content")),
        ("createdAt", get_or_null(n, "createdAt")),
        ("updatedAt", get_or_null(n, "updatedAt")),
        ("releases", Value::Array(releases)),
        ("lastRelease", last),
    ])
}

fn pipeline_out(ctx: &Ctx, p: &Value, details: bool) -> Value {
    let mut o = obj(vec![
        ("id", json!(s(p, "id"))),
        ("name", json!(s(p, "name"))),
        ("slugId", get_or_null(p, "slugId")),
        ("description", get_or_null(p, "description")),
        (
            "type",
            json!(match s(p, "type") {
                "" => "continuous",
                t => t,
            }),
        ),
        (
            "production",
            json!(p["production"].as_bool().unwrap_or(false)),
        ),
        (
            "pathPatterns",
            p.get("pathPatterns").cloned().unwrap_or(json!([])),
        ),
        ("createdAt", get_or_null(p, "createdAt")),
        ("updatedAt", get_or_null(p, "updatedAt")),
        ("archivedAt", get_or_null(p, "archivedAt")),
        (
            "url",
            json!(format!(
                "https://linear.app/{}/releases/{}",
                ctx.url_key(),
                s(p, "slugId")
            )),
        ),
    ]);
    if details {
        let teams: Vec<Value> = p["teamIds"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| ctx.get("teams", t.as_str().unwrap_or("")))
                    .map(|t| json!({"id": s(&t, "id"), "name": s(&t, "name"), "key": s(&t, "key")}))
                    .collect()
            })
            .unwrap_or_default();
        let stages: Vec<Value> = ctx
            .all("release_stages")
            .iter()
            .filter(|st| s(st, "pipelineId") == s(p, "id"))
            .map(|st| stage_out(ctx, st, false))
            .collect();
        let latest = ctx
            .all("release_notes")
            .into_iter()
            .filter(|n| s(n, "pipelineId") == s(p, "id"))
            .max_by(|a, b| s(a, "createdAt").cmp(s(b, "createdAt")))
            .map(|n| note_out(ctx, &n))
            .unwrap_or(Value::Null);
        o["teams"] = Value::Array(teams);
        o["stages"] = Value::Array(stages);
        o["latestReleaseNote"] = latest;
    }
    o
}

fn release_out(ctx: &Ctx, r: &Value, details: bool) -> Value {
    let stage = match ctx.get("release_stages", s(r, "stageId")) {
        Some(st) => {
            json!({"id": s(&st, "id"), "name": s(&st, "name"), "color": get_or_null(&st, "color"), "type": s(&st, "type"), "frozen": st["frozen"].as_bool().unwrap_or(false)})
        }
        None => Value::Null,
    };
    let issues: Vec<Value> = ctx
        .all("issues")
        .iter()
        .filter(|i| {
            i["releaseIds"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == s(r, "id")))
        })
        .map(
            |i| json!({"id": s(i, "id"), "identifier": s(i, "identifier"), "title": s(i, "title")}),
        )
        .collect();
    let mut o = obj(vec![
        ("id", json!(s(r, "id"))),
        ("name", json!(s(r, "name"))),
        ("version", get_or_null(r, "version")),
        ("description", get_or_null(r, "description")),
        ("commitSha", get_or_null(r, "commitSha")),
        ("startDate", get_or_null(r, "startDate")),
        ("targetDate", get_or_null(r, "targetDate")),
        ("startedAt", get_or_null(r, "startedAt")),
        ("completedAt", get_or_null(r, "completedAt")),
        ("canceledAt", get_or_null(r, "canceledAt")),
        ("createdAt", get_or_null(r, "createdAt")),
        ("updatedAt", get_or_null(r, "updatedAt")),
        (
            "trashed",
            r.get("trashed")
                .filter(|t| t.is_boolean())
                .cloned()
                .unwrap_or(Value::Null),
        ),
        (
            "url",
            json!(format!(
                "https://linear.app/{}/release/{}",
                ctx.url_key(),
                s(r, "id")
            )),
        ),
        ("issueCount", json!(issues.len())),
        ("currentProgress", get_or_null(r, "currentProgress")),
        (
            "creator",
            match ctx.get("users", s(r, "creatorId")) {
                Some(u) => {
                    json!({"id": s(&u, "id"), "name": s(&u, "name"), "email": get_or_null(&u, "email")})
                }
                None => Value::Null,
            },
        ),
        ("pipeline", pipeline_ref(ctx, s(r, "pipelineId"))),
        ("stage", stage),
    ]);
    if details {
        let notes: Vec<Value> = ctx
            .all("release_notes")
            .iter()
            .filter(|n| {
                n["releaseIds"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x == s(r, "id")))
            })
            .map(|n| note_out(ctx, n))
            .collect();
        o["issues"] = Value::Array(issues);
        o["releaseNotes"] = Value::Array(notes);
    }
    o
}

pub fn get_pipelines(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        listed(ctx, "release_pipelines", args, 25)
            .iter()
            .map(|p| pipeline_out(ctx, p, false))
            .collect(),
    ))
}

pub fn get_pipeline(ctx: &mut Ctx, args: &Value) -> R {
    let p = ctx.need(
        "release_pipelines",
        arg(args, "id").unwrap_or(""),
        "ReleasePipeline",
    )?;
    Ok(pipeline_out(ctx, &p, true))
}

fn pipeline_fields(ctx: &Ctx, args: &Value, p: &mut Value) -> Result<bool, String> {
    let mut any = false;
    if let Some(n) = arg(args, "name") {
        p["name"] = json!(n);
        any = true;
    }
    if let Some(t) = arg_strs(args, "teamIds") {
        for x in &t {
            ctx.need("teams", x, "Team")?;
        }
        p["teamIds"] = json!(t);
        any = true;
    }
    for (f, k) in [("type", "type"), ("slugId", "slugId")] {
        if let Some(v) = arg(args, f) {
            p[k] = json!(v);
            any = true;
        }
    }
    if let Some(b) = arg_bool(args, "isProduction") {
        p["production"] = json!(b);
        any = true;
    }
    if let Some(pp) = arg_strs(args, "includePathPatterns") {
        p["pathPatterns"] = json!(pp);
        any = true;
    }
    Ok(any)
}

pub fn create_pipeline(ctx: &mut Ctx, args: &Value) -> R {
    if arg(args, "name").is_none() {
        return Err("Release pipeline name is required".into());
    }
    let id = ctx.uuid("release_pipeline");
    let slug = ctx.slug_id("pipeline_slug");
    let now = ctx.iso_now();
    let mut p = json!({"id": id, "slugId": slug, "type": "continuous", "production": false, "pathPatterns": [], "createdAt": now, "updatedAt": now});
    pipeline_fields(ctx, args, &mut p)?;
    ctx.put("release_pipelines", &id, p.clone());
    Ok(pipeline_out(ctx, &p, true))
}

pub fn update_pipeline(ctx: &mut Ctx, args: &Value) -> R {
    let mut p = ctx.need(
        "release_pipelines",
        arg(args, "id").unwrap_or(""),
        "ReleasePipeline",
    )?;
    if !pipeline_fields(ctx, args, &mut p)? {
        return Err("At least one release pipeline field must be provided".into());
    }
    p["updatedAt"] = json!(ctx.iso_now());
    ctx.save("release_pipelines", p.clone());
    Ok(pipeline_out(ctx, &p, true))
}

pub fn archive_pipeline(ctx: &mut Ctx, args: &Value, archive: bool) -> R {
    let mut p = ctx.need(
        "release_pipelines",
        arg(args, "pipelineId").unwrap_or(""),
        "ReleasePipeline",
    )?;
    p["archivedAt"] = if archive {
        json!(ctx.iso_now())
    } else {
        Value::Null
    };
    ctx.save("release_pipelines", p.clone());
    Ok(json!({"success": true, "releasePipeline": pipeline_out(ctx, &p, true)}))
}

pub fn delete_pipeline(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "pipelineId").unwrap_or("").to_owned();
    ctx.need("release_pipelines", &id, "ReleasePipeline")?;
    ctx.remove("release_pipelines", &id);
    Ok(json!({"success": true, "id": id}))
}

fn release_filter(args: &Value, r: &Value) -> bool {
    arg(args, "pipelineId").is_none_or(|p| s(r, "pipelineId") == p)
        && arg(args, "stageId").is_none_or(|st| s(r, "stageId") == st)
}

pub fn get_releases(ctx: &mut Ctx, args: &Value) -> R {
    let list: Vec<Value> = order(args, ctx.all("releases"))
        .into_iter()
        .filter(|r| archived_ok(args, r) && release_filter(args, r))
        .take(limit(args, 25))
        .map(|r| release_out(ctx, &r, false))
        .collect();
    Ok(Value::Array(list))
}

pub fn get_release(ctx: &mut Ctx, args: &Value) -> R {
    let r = ctx.need("releases", arg(args, "id").unwrap_or(""), "Release")?;
    Ok(release_out(ctx, &r, true))
}

pub fn search_releases(ctx: &mut Ctx, args: &Value) -> R {
    let term = arg(args, "term").map(str::to_lowercase);
    let list: Vec<Value> = ctx
        .all("releases")
        .into_iter()
        .filter(|r| archived_ok(args, r) && release_filter(args, r))
        .filter(|r| {
            term.as_ref().is_none_or(|t| {
                format!(
                    "{} {} {}",
                    s(r, "name"),
                    s(r, "version"),
                    s(r, "description")
                )
                .to_lowercase()
                .contains(t.as_str())
            })
        })
        .take(limit(args, 20))
        .map(|r| release_out(ctx, &r, false))
        .collect();
    Ok(Value::Array(list))
}

pub fn get_stages(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        listed(ctx, "release_stages", args, 25)
            .iter()
            .filter(|st| arg(args, "pipelineId").is_none_or(|p| s(st, "pipelineId") == p))
            .map(|st| stage_out(ctx, st, true))
            .collect(),
    ))
}

pub fn create_stage(ctx: &mut Ctx, args: &Value) -> R {
    let (Some(pid), Some(_), Some(_)) = (
        arg(args, "pipelineId"),
        arg(args, "name"),
        arg(args, "color"),
    ) else {
        return Err("Release stage pipeline ID, name, and color are required".into());
    };
    ctx.need("release_pipelines", pid, "ReleasePipeline")?;
    let id = arg(args, "id")
        .map(str::to_owned)
        .unwrap_or_else(|| ctx.uuid("release_stage"));
    let now = ctx.iso_now();
    let doc = json!({
        "id": id, "pipelineId": pid, "name": s(args, "name"), "color": s(args, "color"),
        "position": args.get("position").cloned().unwrap_or(json!(0)), "type": s(args, "type"),
        "frozen": arg_bool(args, "frozen").unwrap_or(false), "createdAt": now, "updatedAt": now,
    });
    ctx.put("release_stages", &id, doc.clone());
    Ok(stage_out(ctx, &doc, true))
}

pub fn update_stage(ctx: &mut Ctx, args: &Value) -> R {
    let mut st = ctx.need(
        "release_stages",
        arg(args, "id").unwrap_or(""),
        "ReleaseStage",
    )?;
    let mut any = false;
    for f in ["name", "color"] {
        if let Some(v) = arg(args, f) {
            st[f] = json!(v);
            any = true;
        }
    }
    if let Some(p) = args.get("position").filter(|x| x.is_number()) {
        st["position"] = p.clone();
        any = true;
    }
    if let Some(f) = arg_bool(args, "frozen") {
        st["frozen"] = json!(f);
        any = true;
    }
    if !any {
        return Err("At least one release stage field must be provided".into());
    }
    st["updatedAt"] = json!(ctx.iso_now());
    ctx.save("release_stages", st.clone());
    Ok(stage_out(ctx, &st, true))
}

pub fn archive_stage(ctx: &mut Ctx, args: &Value, archive: bool) -> R {
    let mut st = ctx.need(
        "release_stages",
        arg(args, "stageId").unwrap_or(""),
        "ReleaseStage",
    )?;
    st["archivedAt"] = if archive {
        json!(ctx.iso_now())
    } else {
        Value::Null
    };
    ctx.save("release_stages", st.clone());
    Ok(json!({"success": true, "releaseStage": stage_out(ctx, &st, true)}))
}

pub fn get_notes(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        listed(ctx, "release_notes", args, 25)
            .iter()
            .map(|n| note_out(ctx, n))
            .collect(),
    ))
}

pub fn get_note(ctx: &mut Ctx, args: &Value) -> R {
    let n = ctx.need(
        "release_notes",
        arg(args, "id").unwrap_or(""),
        "ReleaseNote",
    )?;
    Ok(note_out(ctx, &n))
}

fn note_releases(ctx: &Ctx, args: &Value, pipeline: &str) -> Result<Option<Vec<String>>, String> {
    let ids = arg_strs(args, "releaseIds").filter(|x| !x.is_empty());
    let from = arg(args, "rangeFromReleaseId");
    let to = arg(args, "rangeToReleaseId");
    if ids.is_some() && (from.is_some() || to.is_some()) {
        return Err(
            "A release note cannot specify both explicit releaseIds and a release range".into(),
        );
    }
    if let Some(ids) = ids {
        for r in &ids {
            ctx.need("releases", r, "Release")?;
        }
        return Ok(Some(ids));
    }
    match (from, to) {
        (Some(a), Some(b)) => {
            let ra = ctx.need("releases", a, "Release")?;
            let rb = ctx.need("releases", b, "Release")?;
            let (lo, hi) = (
                s(&ra, "createdAt").to_owned(),
                s(&rb, "createdAt").to_owned(),
            );
            Ok(Some(
                ctx.all("releases")
                    .iter()
                    .filter(|r| {
                        (pipeline.is_empty() || s(r, "pipelineId") == pipeline)
                            && s(r, "createdAt") >= lo.as_str()
                            && s(r, "createdAt") <= hi.as_str()
                    })
                    .map(|r| s(r, "id").to_owned())
                    .collect(),
            ))
        }
        (None, None) => Ok(None),
        _ => Err(
            "A release note range update must include both rangeFromReleaseId and rangeToReleaseId"
                .into(),
        ),
    }
}

pub fn create_note(ctx: &mut Ctx, args: &Value) -> R {
    let pid = arg(args, "pipelineId")
        .ok_or_else(|| "Release pipeline ID is required".to_owned())?
        .to_owned();
    ctx.need("release_pipelines", &pid, "ReleasePipeline")?;
    let releases = match note_releases(ctx, args, &pid) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Err(
                "A release note must specify either releaseIds or a complete release range".into(),
            );
        }
        Err(e) if e.contains("range update") => {
            return Err(
                "A release note must specify either releaseIds or a complete release range".into(),
            );
        }
        Err(e) => return Err(e),
    };
    let id = ctx.uuid("release_note");
    let slug = ctx.slug_id("note_slug");
    let now = ctx.iso_now();
    let title = releases
        .last()
        .and_then(|r| ctx.get("releases", r))
        .map(|r| s(&r, "name").to_owned());
    let doc = json!({"id": id, "pipelineId": pid, "title": title, "slugId": slug, "content": arg(args, "content"), "releaseIds": releases, "createdAt": now, "updatedAt": now});
    ctx.put("release_notes", &id, doc.clone());
    Ok(note_out(ctx, &doc))
}

pub fn update_note(ctx: &mut Ctx, args: &Value) -> R {
    let mut n = ctx.need(
        "release_notes",
        arg(args, "id").unwrap_or(""),
        "ReleaseNote",
    )?;
    let releases = note_releases(ctx, args, s(&n, "pipelineId"))?;
    let content = arg(args, "content");
    if releases.is_none() && content.is_none() {
        return Err("At least one release note field must be provided".into());
    }
    if let Some(c) = content {
        n["content"] = json!(c);
    }
    if let Some(r) = releases {
        n["releaseIds"] = json!(r);
    }
    n["updatedAt"] = json!(ctx.iso_now());
    ctx.save("release_notes", n.clone());
    Ok(note_out(ctx, &n))
}

pub fn delete_note(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    ctx.need("release_notes", &id, "ReleaseNote")?;
    ctx.remove("release_notes", &id);
    Ok(json!({"success": true, "id": id}))
}

const RELEASE_FIELDS: [&str; 9] = [
    "name",
    "version",
    "description",
    "commitSha",
    "stageId",
    "startDate",
    "targetDate",
    "startedAt",
    "completedAt",
];

pub fn create_release(ctx: &mut Ctx, args: &Value) -> R {
    let (Some(pid), Some(_)) = (arg(args, "pipelineId"), arg(args, "name")) else {
        return Err("Release pipeline ID and name are required".into());
    };
    ctx.need("release_pipelines", pid, "ReleasePipeline")?;
    if let Some(st) = arg(args, "stageId") {
        ctx.need("release_stages", st, "ReleaseStage")?;
    }
    let id = ctx.uuid("release");
    let now = ctx.iso_now();
    let mut r = json!({"id": id, "pipelineId": pid, "createdAt": arg(args, "createdAt").and_then(crate::store::norm_time).unwrap_or_else(|| now.clone()), "updatedAt": now, "creatorId": ctx.viewer_id(), "trashed": null});
    for f in RELEASE_FIELDS {
        r[f] = arg(args, f).map(|v| json!(v)).unwrap_or(Value::Null);
    }
    if r["stageId"].is_null()
        && let Some(st) = ctx
            .all("release_stages")
            .into_iter()
            .find(|st| s(st, "pipelineId") == pid && s(st, "type") == "planned")
    {
        r["stageId"] = json!(s(&st, "id"));
    }
    ctx.put("releases", &id, r.clone());
    Ok(release_out(ctx, &r, true))
}

pub fn update_release(ctx: &mut Ctx, args: &Value) -> R {
    let mut r = ctx.need("releases", arg(args, "id").unwrap_or(""), "Release")?;
    let mut any = false;
    for f in RELEASE_FIELDS.iter().chain(["pipelineId"].iter()) {
        if let Some(v) = arg(args, f) {
            r[*f] = json!(v);
            any = true;
        }
    }
    if let Some(t) = arg_bool(args, "trashed") {
        r["trashed"] = json!(t);
        any = true;
    }
    if !any {
        return Err("At least one release field must be provided".into());
    }
    r["updatedAt"] = json!(ctx.iso_now());
    ctx.save("releases", r.clone());
    Ok(release_out(ctx, &r, true))
}

pub fn complete_release(ctx: &mut Ctx, args: &Value) -> R {
    let pid = arg(args, "pipelineId")
        .ok_or_else(|| "Release pipeline ID is required".to_owned())?
        .to_owned();
    let done = ctx
        .all("release_stages")
        .into_iter()
        .find(|st| s(st, "pipelineId") == pid && s(st, "type") == "completed" && opt(st, "archivedAt").is_none())
        .ok_or_else(|| {
            format!("Cannot complete release for pipeline {pid}: no completed release stage exists. Create a completed stage in Linear before calling completeRelease.")
        })?;
    let version = arg(args, "version").map(str::to_owned);
    let current = ctx.all("releases").into_iter().find(|r| {
        s(r, "pipelineId") == pid
            && opt(r, "completedAt").is_none()
            && version.as_ref().is_none_or(|v| s(r, "version") == v)
    });
    let now = ctx.iso_now();
    let mut r = match current {
        Some(r) => r,
        None => {
            let id = ctx.uuid("release");
            json!({"id": id, "pipelineId": pid, "name": version.clone().unwrap_or_else(|| "Release".into()), "version": version, "createdAt": now, "creatorId": ctx.viewer_id()})
        }
    };
    r["stageId"] = json!(s(&done, "id"));
    r["completedAt"] = json!(now);
    r["updatedAt"] = json!(now);
    ctx.save("releases", r.clone());
    Ok(release_out(ctx, &r, true))
}

pub fn archive_release(ctx: &mut Ctx, args: &Value, archive: bool) -> R {
    let mut r = ctx.need("releases", arg(args, "releaseId").unwrap_or(""), "Release")?;
    r["archivedAt"] = if archive {
        json!(ctx.iso_now())
    } else {
        Value::Null
    };
    ctx.save("releases", r.clone());
    Ok(json!({"success": true, "release": release_out(ctx, &r, true)}))
}

pub fn issue_release(ctx: &mut Ctx, args: &Value, add: bool) -> R {
    let raw_issue = arg(args, "issueId").unwrap_or("").to_owned();
    let rid = arg(args, "releaseId").unwrap_or("").to_owned();
    let (key, mut issue) = ctx.need_issue(&raw_issue)?;
    let r = ctx.need("releases", &rid, "Release")?;
    let mut ids: Vec<String> = issue["releaseIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if add {
        if !ids.contains(&rid) {
            ids.push(rid.clone());
        }
    } else if !ids.contains(&rid) {
        return Err(not_found("IssueToRelease"));
    } else {
        ids.retain(|x| *x != rid);
    }
    issue["releaseIds"] = json!(ids);
    ctx.put("issues", &key, issue.clone());
    if add {
        Ok(
            json!({"success": true, "issue": {"id": s(&issue, "id"), "identifier": s(&issue, "identifier"), "title": s(&issue, "title")}, "release": {"id": s(&r, "id"), "name": s(&r, "name"), "version": get_or_null(&r, "version")}}),
        )
    } else {
        Ok(
            json!({"success": true, "issue": {"id": raw_issue, "identifier": raw_issue, "title": ""}, "release": {"id": rid, "name": "", "version": null}}),
        )
    }
}

// ───────────────────────────── customers ─────────────────────────────

fn status_out(st: &Value) -> Value {
    obj(vec![
        ("id", json!(s(st, "id"))),
        ("name", json!(s(st, "name"))),
        (
            "displayName",
            json!(match s(st, "displayName") {
                "" => s(st, "name"),
                d => d,
            }),
        ),
        ("type", get_or_null(st, "type")),
        ("color", get_or_null(st, "color")),
        ("description", get_or_null(st, "description")),
        ("position", get_or_null(st, "position")),
        ("createdAt", get_or_null(st, "createdAt")),
        ("updatedAt", get_or_null(st, "updatedAt")),
        ("archivedAt", get_or_null(st, "archivedAt")),
    ])
}

fn customer_out(ctx: &Ctx, c: &Value) -> Value {
    let needs = ctx
        .all("customer_needs")
        .iter()
        .filter(|n| s(n, "customerId") == s(c, "id"))
        .count();
    let mut st = ctx
        .get("customer_statuses", s(c, "statusId"))
        .map(|x| status_out(&x))
        .unwrap_or(Value::Null);
    let tier = ctx.get("customer_tiers", s(c, "tierId")).map(|x| {
        let mut t = status_out(&x);
        t.as_object_mut().expect("object").remove("type");
        t
    });
    if st.is_object() && st["type"].is_null() {
        st["type"] = Value::Null;
    }
    obj(vec![
        ("id", json!(s(c, "id"))),
        ("name", json!(s(c, "name"))),
        ("slugId", get_or_null(c, "slugId")),
        ("domains", c.get("domains").cloned().unwrap_or(json!([]))),
        (
            "externalIds",
            c.get("externalIds").cloned().unwrap_or(json!([])),
        ),
        ("logoUrl", get_or_null(c, "logoUrl")),
        ("mainSourceId", get_or_null(c, "mainSourceId")),
        ("revenue", get_or_null(c, "revenue")),
        ("size", get_or_null(c, "size")),
        ("slackChannelId", get_or_null(c, "slackChannelId")),
        ("approximateNeedCount", json!(needs)),
        ("createdAt", json!(s(c, "createdAt"))),
        ("updatedAt", json!(s(c, "updatedAt"))),
        ("archivedAt", get_or_null(c, "archivedAt")),
        (
            "url",
            json!(format!(
                "https://linear.app/{}/customer/{}",
                ctx.url_key(),
                s(c, "slugId")
            )),
        ),
        ("owner", ctx.simple_user(s(c, "ownerId"))),
        ("status", st),
        ("tier", tier.unwrap_or(Value::Null)),
    ])
}

fn need_out(ctx: &Ctx, n: &Value) -> Value {
    let issue = ctx.issue_ref(s(n, "issueId"));
    let original = ctx.issue_ref(s(n, "originalIssueId"));
    obj(vec![
        ("id", json!(s(n, "id"))),
        ("body", get_or_null(n, "body")),
        ("content", get_or_null(n, "content")),
        ("priority", get_or_null(n, "priority")),
        ("url", get_or_null(n, "url")),
        ("attachmentId", get_or_null(n, "attachmentId")),
        ("commentId", get_or_null(n, "commentId")),
        ("customerId", get_or_null(n, "customerId")),
        (
            "issueId",
            if issue.is_null() {
                Value::Null
            } else {
                issue["id"].clone()
            },
        ),
        (
            "originalIssueId",
            if original.is_null() {
                Value::Null
            } else {
                original["id"].clone()
            },
        ),
        ("projectId", get_or_null(n, "projectId")),
        ("createdAt", json!(s(n, "createdAt"))),
        ("updatedAt", json!(s(n, "updatedAt"))),
        ("archivedAt", get_or_null(n, "archivedAt")),
        ("customer", ctx.named_ref("customers", s(n, "customerId"))),
        ("issue", issue),
        ("originalIssue", original),
        ("project", ctx.named_ref("projects", s(n, "projectId"))),
        ("creator", ctx.simple_user(s(n, "creatorId"))),
        ("projectAttachment", Value::Null),
    ])
}

pub fn get_customers(ctx: &mut Ctx, args: &Value) -> R {
    let name = arg(args, "name").map(str::to_lowercase);
    let domain = arg(args, "domain").map(str::to_lowercase);
    let list: Vec<Value> = order(args, ctx.all("customers"))
        .into_iter()
        .filter(|c| archived_ok(args, c))
        .filter(|c| {
            name.as_ref()
                .is_none_or(|n| s(c, "name").to_lowercase().contains(n.as_str()))
        })
        .filter(|c| {
            domain.as_ref().is_none_or(|d| {
                c["domains"].as_array().is_some_and(|a| {
                    a.iter()
                        .any(|x| x.as_str().unwrap_or("").to_lowercase() == *d)
                })
            })
        })
        .filter(|c| arg(args, "statusId").is_none_or(|v| s(c, "statusId") == v))
        .filter(|c| arg(args, "tierId").is_none_or(|v| s(c, "tierId") == v))
        .filter(|c| arg(args, "ownerId").is_none_or(|v| s(c, "ownerId") == v))
        .take(limit(args, 25))
        .map(|c| customer_out(ctx, &c))
        .collect();
    Ok(Value::Array(list))
}

pub fn get_customer(ctx: &mut Ctx, args: &Value) -> R {
    let c = ctx.need("customers", arg(args, "id").unwrap_or(""), "Customer")?;
    Ok(customer_out(ctx, &c))
}

fn customer_fields(ctx: &Ctx, args: &Value, c: &mut Value) -> Result<bool, String> {
    let mut any = false;
    for f in ["name", "logoUrl", "mainSourceId", "slackChannelId"] {
        if let Some(v) = arg(args, f) {
            c[f] = json!(v);
            any = true;
        }
    }
    for (f, coll, kind) in [
        ("ownerId", "users", "User"),
        ("statusId", "customer_statuses", "CustomerStatus"),
        ("tierId", "customer_tiers", "CustomerTier"),
    ] {
        if let Some(v) = arg(args, f) {
            ctx.need(coll, v, kind)?;
            c[f] = json!(v);
            any = true;
        }
    }
    for f in ["domains", "externalIds"] {
        if let Some(v) = arg_strs(args, f).filter(|x| !x.is_empty()) {
            c[f] = json!(v);
            any = true;
        }
    }
    for f in ["revenue", "size"] {
        if let Some(v) = args.get(f).filter(|x| x.is_number()) {
            c[f] = v.clone();
            any = true;
        }
    }
    Ok(any)
}

pub fn create_customer(ctx: &mut Ctx, args: &Value) -> R {
    let id = ctx.uuid("customer");
    let slug = ctx.slug_id("customer_slug");
    let now = ctx.iso_now();
    let mut c = json!({"id": id, "slugId": slug, "domains": [], "externalIds": [], "createdAt": now, "updatedAt": now});
    customer_fields(ctx, args, &mut c)?;
    ctx.put("customers", &id, c.clone());
    Ok(customer_out(ctx, &c))
}

pub fn update_customer(ctx: &mut Ctx, args: &Value) -> R {
    let mut c = ctx.need("customers", arg(args, "id").unwrap_or(""), "Customer")?;
    if !customer_fields(ctx, args, &mut c)? {
        return Err("At least one customer field must be provided".into());
    }
    c["updatedAt"] = json!(ctx.iso_now());
    ctx.save("customers", c.clone());
    Ok(customer_out(ctx, &c))
}

pub fn delete_customer(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    ctx.need("customers", &id, "Customer")?;
    ctx.remove("customers", &id);
    Ok(json!({"success": true, "id": id}))
}

pub fn get_needs(ctx: &mut Ctx, args: &Value) -> R {
    let issue_id = arg(args, "issueId").map(|i| {
        ctx.find_issue(i)
            .map(|(_, x)| s(&x, "id").to_owned())
            .unwrap_or_else(|| i.to_owned())
    });
    let list: Vec<Value> = order(args, ctx.all("customer_needs"))
        .into_iter()
        .filter(|n| archived_ok(args, n))
        .filter(|n| arg(args, "customerId").is_none_or(|v| s(n, "customerId") == v))
        .filter(|n| issue_id.as_ref().is_none_or(|v| s(n, "issueId") == v))
        .filter(|n| arg(args, "projectId").is_none_or(|v| s(n, "projectId") == v))
        .filter(|n| arg_i64(args, "priority").is_none_or(|p| n["priority"].as_i64() == Some(p)))
        .take(limit(args, 25))
        .map(|n| need_out(ctx, &n))
        .collect();
    Ok(Value::Array(list))
}

pub fn get_need(ctx: &mut Ctx, args: &Value) -> R {
    let n = ctx.need(
        "customer_needs",
        arg(args, "id").unwrap_or(""),
        "CustomerNeed",
    )?;
    Ok(need_out(ctx, &n))
}

fn need_fields(ctx: &Ctx, args: &Value, n: &mut Value) -> Result<bool, String> {
    let mut any = false;
    if let Some(b) = arg(args, "body") {
        n["body"] = json!(b);
        any = true;
    }
    if let Some(u) = arg(args, "attachmentUrl") {
        n["url"] = json!(u);
        any = true;
    }
    if let Some(c) = arg(args, "customerId") {
        ctx.need("customers", c, "Customer")?;
        n["customerId"] = json!(c);
        any = true;
    }
    if let Some(ext) = arg(args, "customerExternalId") {
        let c = ctx
            .all("customers")
            .into_iter()
            .find(|c| {
                c["externalIds"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x == ext))
            })
            .ok_or_else(|| not_found("Customer"))?;
        n["customerId"] = json!(s(&c, "id"));
        any = true;
    }
    if let Some(i) = arg(args, "issueId") {
        let (_, issue) = ctx.need_issue(i)?;
        n["issueId"] = json!(s(&issue, "id"));
        any = true;
    }
    if let Some(p) = arg(args, "projectId") {
        ctx.need("projects", p, "Project")?;
        n["projectId"] = json!(p);
        any = true;
    }
    if let Some(p) = args.get("priority").filter(|x| x.is_number()) {
        n["priority"] = p.clone();
        any = true;
    }
    Ok(any)
}

pub fn create_need(ctx: &mut Ctx, args: &Value) -> R {
    if arg(args, "issueId").is_none() && arg(args, "projectId").is_none() {
        return Err("Customer needs require issueId or projectId".into());
    }
    let id = ctx.uuid("customer_need");
    let now = ctx.iso_now();
    let mut n = json!({"id": id, "createdAt": arg(args, "createdAt").and_then(crate::store::norm_time).unwrap_or_else(|| now.clone()), "updatedAt": now, "creatorId": ctx.viewer_id(), "priority": 0});
    need_fields(ctx, args, &mut n)?;
    for f in ["attachmentId", "commentId"] {
        if let Some(v) = arg(args, f) {
            n[f] = json!(v);
        }
    }
    ctx.put("customer_needs", &id, n.clone());
    Ok(need_out(ctx, &n))
}

pub fn create_need_from_attachment(ctx: &mut Ctx, args: &Value) -> R {
    let att = ctx.need(
        "attachments",
        arg(args, "attachmentId").unwrap_or(""),
        "Attachment",
    )?;
    let mut a = args.clone();
    a["issueId"] = json!(s(&att, "issueId"));
    let mut out = create_need(ctx, &a)?;
    out["attachmentId"] = json!(s(&att, "id"));
    let id = s(&out, "id").to_owned();
    if let Some(mut n) = ctx.get("customer_needs", &id) {
        n["attachmentId"] = json!(s(&att, "id"));
        ctx.save("customer_needs", n);
    }
    Ok(out)
}

pub fn update_need(ctx: &mut Ctx, args: &Value) -> R {
    let mut n = ctx.need(
        "customer_needs",
        arg(args, "id").unwrap_or(""),
        "CustomerNeed",
    )?;
    if !need_fields(ctx, args, &mut n)? && arg_bool(args, "applyPriorityToRelatedNeeds").is_none() {
        return Err("At least one customer need field must be provided".into());
    }
    n["updatedAt"] = json!(ctx.iso_now());
    ctx.save("customer_needs", n.clone());
    let mut out = need_out(ctx, &n);
    out["updatedRelatedNeeds"] = json!([]);
    Ok(out)
}

pub fn archive_need(ctx: &mut Ctx, args: &Value, archive: Option<bool>) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    let mut n = ctx.need("customer_needs", &id, "CustomerNeed")?;
    match archive {
        Some(a) => {
            n["archivedAt"] = if a { json!(ctx.iso_now()) } else { Value::Null };
            ctx.save("customer_needs", n);
        }
        None => {
            ctx.remove("customer_needs", &id);
        }
    }
    Ok(json!({"success": true, "id": id}))
}

pub fn get_customer_statuses(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        listed(ctx, "customer_statuses", args, 25)
            .iter()
            .map(status_out)
            .collect(),
    ))
}

pub fn get_customer_tiers(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        listed(ctx, "customer_tiers", args, 25)
            .iter()
            .map(|t| {
                let mut o = status_out(t);
                o.as_object_mut().expect("object").remove("type");
                o
            })
            .collect(),
    ))
}

// ───────────────────────────── webhooks ─────────────────────────────

fn webhook_out(ctx: &Ctx, w: &Value) -> Value {
    obj(vec![
        ("id", json!(s(w, "id"))),
        ("label", get_or_null(w, "label")),
        ("url", get_or_null(w, "url")),
        ("enabled", json!(w["enabled"].as_bool().unwrap_or(true))),
        (
            "allPublicTeams",
            json!(w["allPublicTeams"].as_bool().unwrap_or(false)),
        ),
        (
            "resourceTypes",
            w.get("resourceTypes").cloned().unwrap_or(json!([])),
        ),
        ("createdAt", json!(s(w, "createdAt"))),
        ("updatedAt", json!(s(w, "updatedAt"))),
        ("archivedAt", get_or_null(w, "archivedAt")),
        ("team", ctx.team_ref_key(s(w, "teamId"))),
        ("creator", ctx.user_ref_email(s(w, "creatorId"))),
    ])
}

pub fn get_webhook(ctx: &mut Ctx, args: &Value) -> R {
    let w = ctx.need("webhooks", arg(args, "id").unwrap_or(""), "Webhook")?;
    Ok(webhook_out(ctx, &w))
}

pub fn get_webhooks(ctx: &mut Ctx, args: &Value) -> R {
    if let Some(t) = arg(args, "teamId") {
        ctx.need("teams", t, "Team")?;
    }
    Ok(Value::Array(
        listed(ctx, "webhooks", args, 25)
            .iter()
            .filter(|w| arg(args, "teamId").is_none_or(|t| s(w, "teamId") == t))
            .map(|w| webhook_out(ctx, w))
            .collect(),
    ))
}

fn public_url(u: &str) -> bool {
    u.starts_with("https://") && !u.contains("localhost") && !u.contains("127.0.0.1")
}

pub fn create_webhook(ctx: &mut Ctx, args: &Value) -> R {
    let url = arg(args, "url").unwrap_or("");
    if !public_url(url) || arg_strs(args, "resourceTypes").is_none_or(|r| r.is_empty()) {
        return Err("Invalid arguments for createWebhook".into());
    }
    if let Some(t) = arg(args, "teamId") {
        ctx.need("teams", t, "Team")?;
    }
    let id = ctx.uuid("webhook");
    let now = ctx.iso_now();
    let doc = json!({
        "id": id, "url": url, "label": arg(args, "label"), "enabled": arg_bool(args, "enabled").unwrap_or(true),
        "allPublicTeams": arg_bool(args, "allPublicTeams").unwrap_or(arg(args, "teamId").is_none()),
        "resourceTypes": args["resourceTypes"], "teamId": arg(args, "teamId"), "creatorId": ctx.viewer_id(),
        "createdAt": now, "updatedAt": now,
    });
    ctx.put("webhooks", &id, doc.clone());
    Ok(webhook_out(ctx, &doc))
}

pub fn update_webhook(ctx: &mut Ctx, args: &Value) -> R {
    let mut w = ctx.need("webhooks", arg(args, "id").unwrap_or(""), "Webhook")?;
    let mut any = false;
    if let Some(u) = arg(args, "url") {
        if !public_url(u) {
            return Err("Invalid arguments for updateWebhook".into());
        }
        w["url"] = json!(u);
        any = true;
    }
    match args.get("label") {
        Some(Value::Null) => {
            w["label"] = Value::Null;
            any = true;
        }
        Some(Value::String(l)) if !l.is_empty() => {
            w["label"] = json!(l);
            any = true;
        }
        _ => {}
    }
    if let Some(e) = arg_bool(args, "enabled") {
        w["enabled"] = json!(e);
        any = true;
    }
    if let Some(r) = arg_strs(args, "resourceTypes").filter(|x| !x.is_empty()) {
        w["resourceTypes"] = json!(r);
        any = true;
    }
    if arg(args, "secret").is_some() {
        any = true;
    }
    if !any {
        return Err("At least one webhook field must be provided".into());
    }
    w["updatedAt"] = json!(ctx.iso_now());
    ctx.save("webhooks", w.clone());
    Ok(webhook_out(ctx, &w))
}

pub fn rotate_webhook_secret(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    ctx.need("webhooks", &id, "Webhook")?;
    let secret = format!(
        "lin_wh_{}",
        ctx.st.ids.next_prefixed("webhook_secret", "", 32)
    );
    Ok(json!({"success": true, "id": id, "secret": secret}))
}

pub fn delete_webhook(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    ctx.need("webhooks", &id, "Webhook")?;
    ctx.remove("webhooks", &id);
    Ok(json!({"success": true, "id": id}))
}

// ───────────────────────────── notifications & subscriptions ─────────────────────────────

fn notification_out(ctx: &Ctx, n: &Value) -> Value {
    obj(vec![
        ("id", json!(s(n, "id"))),
        ("type", json!(s(n, "type"))),
        ("title", json!(s(n, "title"))),
        ("subtitle", json!(s(n, "subtitle"))),
        ("url", json!(s(n, "url"))),
        ("readAt", get_or_null(n, "readAt")),
        ("snoozedUntilAt", get_or_null(n, "snoozedUntilAt")),
        ("createdAt", json!(s(n, "createdAt"))),
        ("updatedAt", json!(s(n, "updatedAt"))),
        ("actor", ctx.user_ref_email(s(n, "actorId"))),
    ])
}

fn my_notifications(ctx: &Ctx, args: &Value, default: i64) -> Vec<Value> {
    let viewer = ctx.viewer_id();
    order(
        args,
        ctx.all("notifications")
            .into_iter()
            .filter(|n| s(n, "userId").is_empty() || s(n, "userId") == viewer)
            .filter(|n| archived_ok(args, n))
            .collect(),
    )
    .into_iter()
    .take(limit(args, default))
    .collect()
}

pub fn get_notifications(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        my_notifications(ctx, args, 25)
            .iter()
            .map(|n| notification_out(ctx, n))
            .collect(),
    ))
}

pub fn mark_read(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    let mut n = ctx.need("notifications", &id, "Notification")?;
    n["readAt"] = json!(ctx.iso_now());
    ctx.save("notifications", n);
    Ok(json!({"success": true, "id": id}))
}

pub fn mark_all_read(ctx: &mut Ctx, args: &Value) -> R {
    let unread: Vec<Value> = my_notifications(ctx, args, 100)
        .into_iter()
        .filter(|n| opt(n, "readAt").is_none())
        .collect();
    let now = ctx.iso_now();
    for mut n in unread.clone() {
        n["readAt"] = json!(now);
        ctx.save("notifications", n);
    }
    Ok(json!({"success": true, "count": unread.len()}))
}

pub fn unread_count(ctx: &mut Ctx, args: &Value) -> R {
    let n = my_notifications(ctx, args, 100)
        .iter()
        .filter(|n| opt(n, "readAt").is_none())
        .count();
    Ok(json!({"count": n}))
}

pub fn get_subscriptions(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        listed(ctx, "notification_subscriptions", args, 25)
            .iter()
            .map(|x| {
                obj(vec![
                    ("id", json!(s(x, "id"))),
                    ("active", json!(x["active"].as_bool().unwrap_or(true))),
                    (
                        "notificationSubscriptionTypes",
                        x.get("notificationSubscriptionTypes")
                            .cloned()
                            .unwrap_or(json!([])),
                    ),
                    ("contextViewType", get_or_null(x, "contextViewType")),
                    ("userContextViewType", get_or_null(x, "userContextViewType")),
                    ("createdAt", json!(s(x, "createdAt"))),
                    ("updatedAt", json!(s(x, "updatedAt"))),
                    ("team", ctx.team_ref(s(x, "teamId"))),
                    ("project", ctx.named_ref("projects", s(x, "projectId"))),
                    ("cycle", ctx.named_ref("cycles", s(x, "cycleId"))),
                    ("label", ctx.named_ref("labels", s(x, "labelId"))),
                    (
                        "initiative",
                        ctx.named_ref("initiatives", s(x, "initiativeId")),
                    ),
                    (
                        "customView",
                        ctx.named_ref("saved_views", s(x, "customViewId")),
                    ),
                    (
                        "subscriber",
                        ctx.user_ref_email(match s(x, "subscriberId") {
                            "" => "",
                            v => v,
                        }),
                    ),
                ])
            })
            .collect(),
    ))
}

// ───────────────────────────── sessions, audit, integrations ─────────────────────────────

pub fn get_sessions(ctx: &mut Ctx, _args: &Value) -> R {
    Ok(Value::Array(
        ctx.all("sessions")
            .iter()
            .map(|x| {
                obj(vec![
                    ("id", json!(s(x, "id"))),
                    ("name", json!(s(x, "name"))),
                    ("client", get_or_null(x, "client")),
                    ("browserType", get_or_null(x, "browserType")),
                    ("operatingSystem", get_or_null(x, "operatingSystem")),
                    ("ip", get_or_null(x, "ip")),
                    ("location", get_or_null(x, "location")),
                    (
                        "isCurrentSession",
                        json!(x["isCurrentSession"].as_bool().unwrap_or(false)),
                    ),
                    (
                        "countryCodes",
                        x.get("countryCodes").cloned().unwrap_or(json!([])),
                    ),
                    ("createdAt", json!(s(x, "createdAt"))),
                    ("updatedAt", json!(s(x, "updatedAt"))),
                    ("lastActiveAt", get_or_null(x, "lastActiveAt")),
                    (
                        "type",
                        json!(match s(x, "type") {
                            "" => "web",
                            t => t,
                        }),
                    ),
                ])
            })
            .collect(),
    ))
}

pub fn logout(ctx: &mut Ctx, args: &Value, which: &str) -> R {
    match which {
        "one" => {
            let id = arg(args, "sessionId").unwrap_or("");
            ctx.need("sessions", id, "AuthenticationSession")?;
            ctx.remove("sessions", id);
        }
        "others" => {
            for x in ctx.all("sessions") {
                if !x["isCurrentSession"].as_bool().unwrap_or(false) {
                    ctx.remove("sessions", s(&x, "id"));
                }
            }
        }
        _ => {
            for x in ctx.all("sessions") {
                ctx.remove("sessions", s(&x, "id"));
            }
        }
    }
    Ok(json!({"success": true}))
}

fn audit_out(ctx: &Ctx, e: &Value) -> Value {
    obj(vec![
        ("id", json!(s(e, "id"))),
        ("type", json!(s(e, "type"))),
        ("actorId", get_or_null(e, "actorId")),
        ("countryCode", get_or_null(e, "countryCode")),
        ("ip", get_or_null(e, "ip")),
        ("metadata", get_or_null(e, "metadata")),
        ("requestInformation", get_or_null(e, "requestInformation")),
        ("createdAt", json!(s(e, "createdAt"))),
        ("updatedAt", json!(s(e, "updatedAt"))),
        ("actor", ctx.user_ref_email(s(e, "actorId"))),
    ])
}

fn require_admin(ctx: &Ctx) -> Result<(), String> {
    let viewer = ctx.get("users", &ctx.viewer_id()).unwrap_or(Value::Null);
    if viewer["admin"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        Err("Forbidden - You need to be an admin to perform this action.".into())
    }
}

pub fn org_audit(ctx: &mut Ctx, args: &Value) -> R {
    require_admin(ctx)?;
    Ok(Value::Array(
        listed(ctx, "audit_entries", args, 25)
            .iter()
            .map(|e| audit_out(ctx, e))
            .collect(),
    ))
}

pub fn user_audit(ctx: &mut Ctx, args: &Value) -> R {
    require_admin(ctx)?;
    let uid = arg(args, "userId").unwrap_or("").to_owned();
    let list: Vec<Value> = order(
        args,
        ctx.all("audit_entries")
            .into_iter()
            .filter(|e| s(e, "actorId") == uid)
            .collect(),
    )
    .into_iter()
    .take(limit(args, 25))
    .map(|e| audit_out(ctx, &e))
    .collect();
    Ok(Value::Array(list))
}

pub fn integrations(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        listed(ctx, "integrations", args, 25)
            .iter()
            .map(|i| {
                obj(vec![
                    ("id", json!(s(i, "id"))),
                    ("service", json!(s(i, "service"))),
                    ("createdAt", json!(s(i, "createdAt"))),
                    ("updatedAt", json!(s(i, "updatedAt"))),
                    ("archivedAt", get_or_null(i, "archivedAt")),
                    ("creator", ctx.user_ref_email(s(i, "creatorId"))),
                    ("team", ctx.team_ref_key(s(i, "teamId"))),
                ])
            })
            .collect(),
    ))
}

// ───────────────────────────── saved views & favorites ─────────────────────────────

fn view_out(ctx: &Ctx, v: &Value) -> Value {
    let mut o = sdk(vec![
        ("id", json!(s(v, "id"))),
        ("name", json!(s(v, "name"))),
        ("description", get_or_null(v, "description")),
        ("shared", json!(v["shared"].as_bool().unwrap_or(false))),
        ("icon", get_or_null(v, "icon")),
        ("color", get_or_null(v, "color")),
        ("slugId", json!(s(v, "slugId"))),
        (
            "filterData",
            v.get("filterData").cloned().unwrap_or(json!({})),
        ),
        ("filters", v.get("filters").cloned().unwrap_or(json!({}))),
        ("projectFilterData", get_or_null(v, "projectFilterData")),
    ]);
    let m = o.as_object_mut().expect("object");
    m.insert("team".into(), ctx.team_ref(s(v, "teamId")));
    m.insert("owner".into(), ctx.user_ref_email(s(v, "ownerId")));
    m.insert("creator".into(), ctx.user_ref_email(s(v, "creatorId")));
    m.insert("createdAt".into(), json!(s(v, "createdAt")));
    m.insert("updatedAt".into(), json!(s(v, "updatedAt")));
    o
}

pub fn get_views(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        listed(ctx, "saved_views", args, 25)
            .iter()
            .map(|v| view_out(ctx, v))
            .collect(),
    ))
}

fn view_fields(ctx: &Ctx, args: &Value, v: &mut Value) -> Result<bool, String> {
    let mut any = false;
    if let Some(n) = arg(args, "name") {
        v["name"] = json!(n);
        any = true;
    }
    if let Some(sh) = arg_bool(args, "shared") {
        v["shared"] = json!(sh);
        any = true;
    }
    for f in [
        "description",
        "icon",
        "color",
        "teamId",
        "projectId",
        "ownerId",
        "filters",
        "filterData",
        "projectFilterData",
    ] {
        match args.get(f) {
            Some(Value::Null) => {
                v[f] = Value::Null;
                any = true;
            }
            Some(x)
                if x.as_str().is_some_and(|t| !t.trim().is_empty())
                    || x.as_object().is_some_and(|o| !o.is_empty()) =>
            {
                if f == "teamId" {
                    ctx.need("teams", x.as_str().unwrap_or(""), "Team")?;
                }
                if f == "ownerId" {
                    ctx.need("users", x.as_str().unwrap_or(""), "User")?;
                }
                v[f] = x.clone();
                any = true;
            }
            _ => {}
        }
    }
    Ok(any)
}

pub fn create_view(ctx: &mut Ctx, args: &Value) -> R {
    let id = ctx.uuid("saved_view");
    let slug = ctx.slug_id("view_slug");
    let now = ctx.iso_now();
    let mut v = json!({"id": id, "slugId": slug, "shared": false, "ownerId": ctx.viewer_id(), "creatorId": ctx.viewer_id(), "createdAt": now, "updatedAt": now, "filterData": {}, "filters": {}});
    view_fields(ctx, args, &mut v)?;
    ctx.put("saved_views", &id, v.clone());
    Ok(view_out(ctx, &v))
}

pub fn update_view(ctx: &mut Ctx, args: &Value) -> R {
    let mut v = ctx.need("saved_views", arg(args, "id").unwrap_or(""), "CustomView")?;
    if !view_fields(ctx, args, &mut v)? {
        return Err("At least one saved view field must be provided".into());
    }
    v["updatedAt"] = json!(ctx.iso_now());
    ctx.save("saved_views", v.clone());
    Ok(view_out(ctx, &v))
}

pub fn delete_view(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    ctx.need("saved_views", &id, "CustomView")?;
    ctx.remove("saved_views", &id);
    Ok(json!({"success": true, "id": id}))
}

fn favorite_out(ctx: &Ctx, f: &Value) -> Value {
    let view = ctx.get("saved_views", s(f, "customViewId")).map(|v| json!({"id": s(&v, "id"), "name": s(&v, "name"), "slugId": get_or_null(&v, "slugId"), "shared": v["shared"].as_bool().unwrap_or(false)}));
    let team = ctx
        .get("teams", s(f, "predefinedViewTeamId"))
        .map(|t| json!({"id": s(&t, "id"), "name": s(&t, "name")}));
    let mut o = obj(vec![
        ("id", json!(s(f, "id"))),
        (
            "type",
            json!(match s(f, "type") {
                "" => "unknown",
                t => t,
            }),
        ),
        ("sortOrder", get_or_null(f, "sortOrder")),
        ("customView", view.unwrap_or(Value::Null)),
        ("predefinedViewType", get_or_null(f, "predefinedViewType")),
        ("predefinedViewTeam", team.unwrap_or(Value::Null)),
        ("createdAt", get_or_null(f, "createdAt")),
        ("updatedAt", get_or_null(f, "updatedAt")),
    ]);
    if let Some(u) = f.get("url").and_then(Value::as_str) {
        o["url"] = json!(u);
    }
    o
}

pub fn favorite_views(ctx: &mut Ctx, args: &Value) -> R {
    Ok(Value::Array(
        listed(ctx, "favorites", args, 25)
            .iter()
            .filter(|f| !s(f, "customViewId").is_empty() || !s(f, "predefinedViewType").is_empty())
            .map(|f| favorite_out(ctx, f))
            .collect(),
    ))
}

pub fn add_favorite(ctx: &mut Ctx, args: &Value) -> R {
    let entity = arg(args, "entityId").unwrap_or("").to_owned();
    let (ty, field) = if ctx.find_issue(&entity).is_some() {
        ("issue", "issueId")
    } else if ctx.get("projects", &entity).is_some() {
        ("project", "projectId")
    } else if ctx.get("saved_views", &entity).is_some() {
        ("customView", "customViewId")
    } else if ctx.get("documents", &entity).is_some() {
        ("document", "documentId")
    } else if ctx.get("cycles", &entity).is_some() {
        ("cycle", "cycleId")
    } else if ctx.get("labels", &entity).is_some() {
        ("label", "labelId")
    } else if ctx.get("initiatives", &entity).is_some() {
        ("initiative", "initiativeId")
    } else {
        return Err(not_found("Entity"));
    };
    let id = ctx.uuid("favorite");
    let now = ctx.iso_now();
    let sort = ctx.all("favorites").len() as f64;
    let entity_id = if ty == "issue" {
        ctx.find_issue(&entity)
            .map(|(_, i)| s(&i, "id").to_owned())
            .unwrap_or(entity)
    } else {
        entity
    };
    let doc = json!({"id": id, "type": ty, field: entity_id, "sortOrder": sort, "createdAt": now, "updatedAt": now, "userId": ctx.viewer_id()});
    ctx.put("favorites", &id, doc.clone());
    Ok(json!({"success": true, "id": id, "entityId": null, "favorite": favorite_out(ctx, &doc)}))
}

pub fn remove_favorite(ctx: &mut Ctx, args: &Value) -> R {
    let fav = arg(args, "favoriteId");
    let entity = arg(args, "entityId");
    if fav.is_none() && entity.is_none() {
        return Err("A favorite identifier must be provided".into());
    }
    let id = match fav {
        Some(f) => f.to_owned(),
        None => return Err("The favorite removal mutation does not expose an entity identifier argument; use favoriteId for this schema".into()),
    };
    ctx.need("favorites", &id, "Favorite")?;
    ctx.remove("favorites", &id);
    Ok(json!({"success": true, "id": id, "entityId": null, "favorite": null}))
}

// ───────────────────────────── OAuth applications ─────────────────────────────

fn oauth_out(a: &Value) -> Value {
    obj(vec![
        ("id", json!(s(a, "id"))),
        ("clientId", json!(s(a, "clientId"))),
        ("name", json!(s(a, "name"))),
        ("description", get_or_null(a, "description")),
        ("developer", json!(s(a, "developer"))),
        ("developerUrl", json!(s(a, "developerUrl"))),
        (
            "distribution",
            json!(match s(a, "distribution") {
                "" => "private",
                d => d,
            }),
        ),
        (
            "grantTypes",
            a.get("grantTypes")
                .cloned()
                .unwrap_or(json!(["authorization_code"])),
        ),
        ("imageUrl", get_or_null(a, "imageUrl")),
        (
            "redirectUris",
            a.get("redirectUris").cloned().unwrap_or(json!([])),
        ),
        (
            "webhookEnabled",
            json!(a["webhookEnabled"].as_bool().unwrap_or(false)),
        ),
        ("webhookUrl", get_or_null(a, "webhookUrl")),
        (
            "webhookResourceTypes",
            a.get("webhookResourceTypes").cloned().unwrap_or(json!([])),
        ),
        ("createdAt", json!(s(a, "createdAt"))),
        ("updatedAt", json!(s(a, "updatedAt"))),
    ])
}

fn query_escape(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn oauth_setup(_ctx: &mut Ctx, args: &Value) -> R {
    let grants = arg_strs(args, "grantTypes")
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| vec!["authorization_code".into()]);
    let webhook_url = arg(args, "webhookUrl");
    let resource_types = arg_strs(args, "webhookResourceTypes").filter(|r| !r.is_empty());
    if webhook_url.is_some() != resource_types.is_some() {
        return Err(
            "OAuth application webhook URL and resource types must be provided together".into(),
        );
    }
    let mut manifest = serde_json::Map::new();
    manifest.insert(
        "$schema".into(),
        json!("https://linear.app/.well-known/oauth-app-manifest.schema.json"),
    );
    manifest.insert("schemaVersion".into(), json!("1.0.0"));
    manifest.insert(
        "distribution".into(),
        json!(arg(args, "distribution").unwrap_or("private")),
    );
    let mut display = serde_json::Map::new();
    if let Some(d) = arg(args, "description") {
        display.insert("description".into(), json!(d));
    }
    if let Some(i) = arg(args, "imageUrl") {
        display.insert("iconUrl".into(), json!(i));
    }
    if !display.is_empty() {
        manifest.insert("display".into(), Value::Object(display));
    }
    manifest.insert("developer".into(), json!({"name": s(args, "developer")}));
    let mut oauth = serde_json::Map::new();
    oauth.insert("client_name".into(), json!(s(args, "name")));
    if let Some(u) = arg(args, "developerUrl") {
        oauth.insert("client_uri".into(), json!(u));
    }
    oauth.insert(
        "redirect_uris".into(),
        args.get("redirectUris").cloned().unwrap_or(json!([])),
    );
    oauth.insert("grant_types".into(), json!(grants));
    manifest.insert("oauth".into(), Value::Object(oauth));
    if let (Some(u), Some(r)) = (webhook_url, resource_types) {
        manifest.insert("webhook".into(), json!({"enabled": arg_bool(args, "webhookEnabled").unwrap_or(true), "url": u, "resourceTypes": r}));
    }
    let manifest = Value::Object(manifest);
    Ok(json!({
        "manifest": manifest,
        "creationUrl": format!("https://linear.app/settings/api/applications/new?manifest={}", query_escape(&manifest.to_string())),
        "requiresUserConfirmation": true,
    }))
}

pub fn oauth_url(_ctx: &mut Ctx, args: &Value) -> R {
    let mut scopes = vec!["read".to_owned()];
    for sc in arg_strs(args, "scopes").unwrap_or_default() {
        if sc != "read" && !scopes.contains(&sc) {
            scopes.push(sc);
        }
    }
    let mut warnings = Vec::new();
    if arg(args, "state").is_none() {
        warnings.push(
            "No state value was provided; include one to protect the OAuth redirect from CSRF.",
        );
    }
    let mut url = format!(
        "https://linear.app/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}",
        query_escape(s(args, "clientId")),
        query_escape(s(args, "redirectUri")),
        query_escape(&scopes.join(","))
    );
    if let Some(a) = arg(args, "actor") {
        url.push_str(&format!("&actor={}", query_escape(a)));
    }
    if let Some(st) = arg(args, "state") {
        url.push_str(&format!("&state={}", query_escape(st)));
    }
    if arg_bool(args, "promptConsent").unwrap_or(false) {
        url.push_str("&prompt=consent");
    }
    if let Some(c) = arg(args, "codeChallenge") {
        url.push_str(&format!(
            "&code_challenge={}&code_challenge_method=S256",
            query_escape(c)
        ));
    }
    Ok(json!({"authorizationUrl": url, "scopes": scopes, "warnings": warnings}))
}

pub fn oauth_client_token(_ctx: &mut Ctx, _args: &Value) -> R {
    Err("Failed creating OAuth client-credentials token; Linear error details were omitted because the response may contain a one-time secret".into())
}

pub fn oauth_apps(ctx: &mut Ctx, _args: &Value) -> R {
    Ok(Value::Array(
        ctx.all("oauth_applications")
            .iter()
            .map(oauth_out)
            .collect(),
    ))
}

pub fn oauth_app(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("");
    let a = ctx
        .get("oauth_applications", id)
        .ok_or_else(|| format!("Managed OAuth application {id} not found"))?;
    Ok(oauth_out(&a))
}

pub fn create_oauth_app(ctx: &mut Ctx, args: &Value) -> R {
    let id = ctx.uuid("oauth_app");
    let client = ctx
        .st
        .ids
        .next_prefixed("oauth_client", "", 32)
        .to_lowercase();
    let now = ctx.iso_now();
    let doc = json!({
        "id": id, "clientId": client, "name": s(args, "name"), "developer": s(args, "developer"),
        "developerUrl": arg(args, "developerUrl").unwrap_or(""), "description": arg(args, "description"),
        "imageUrl": arg(args, "imageUrl"), "redirectUris": args.get("redirectUris").cloned().unwrap_or(json!([])),
        "grantTypes": arg_strs(args, "grantTypes").filter(|g| !g.is_empty()).unwrap_or_else(|| vec!["authorization_code".into()]),
        "webhookEnabled": arg(args, "webhookUrl").is_some(), "webhookUrl": arg(args, "webhookUrl"),
        "webhookResourceTypes": args.get("webhookResourceTypes").cloned().unwrap_or(json!([])),
        "distribution": "private", "createdAt": now, "updatedAt": now,
    });
    ctx.put("oauth_applications", &id, doc.clone());
    let secret = format!(
        "lin_oauth_{}",
        ctx.st
            .ids
            .next_prefixed("oauth_secret", "", 32)
            .to_lowercase()
    );
    let webhook_secret = if arg(args, "webhookUrl").is_some() {
        json!(format!(
            "lin_wh_{}",
            ctx.st.ids.next_prefixed("oauth_wh_secret", "", 32)
        ))
    } else {
        Value::Null
    };
    Ok(
        json!({"application": oauth_out(&doc), "clientSecret": secret, "webhookSecret": webhook_secret}),
    )
}

pub fn update_oauth_app(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    let mut a = ctx
        .get("oauth_applications", &id)
        .ok_or_else(|| format!("Failed to update managed OAuth application {id}"))?;
    let mut any = false;
    for f in [
        "name",
        "developer",
        "developerUrl",
        "description",
        "imageUrl",
        "webhookUrl",
    ] {
        match args.get(f) {
            Some(Value::Null) => {
                a[f] = Value::Null;
                any = true;
            }
            Some(Value::String(v)) if !v.trim().is_empty() => {
                a[f] = json!(v);
                any = true;
            }
            _ => {}
        }
    }
    for f in ["redirectUris", "grantTypes", "webhookResourceTypes"] {
        if let Some(v) = args
            .get(f)
            .filter(|v| v.as_array().is_some_and(|x| !x.is_empty()))
        {
            a[f] = v.clone();
            any = true;
        }
    }
    if let Some(e) = arg_bool(args, "webhookEnabled") {
        a["webhookEnabled"] = json!(e);
        any = true;
    }
    if !any {
        return Err("At least one managed OAuth application field must be provided".into());
    }
    a["updatedAt"] = json!(ctx.iso_now());
    ctx.save("oauth_applications", a.clone());
    Ok(oauth_out(&a))
}

pub fn archive_oauth_app(ctx: &mut Ctx, args: &Value) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    ctx.get("oauth_applications", &id)
        .ok_or_else(|| format!("Failed to archive managed OAuth application {id}"))?;
    ctx.remove("oauth_applications", &id);
    Ok(json!({"success": true, "id": id}))
}

pub fn rotate_oauth_secret(ctx: &mut Ctx, args: &Value, webhook: bool) -> R {
    let id = arg(args, "id").unwrap_or("").to_owned();
    let action = if webhook {
        "rotating managed OAuth application webhook secret"
    } else {
        "rotating managed OAuth application secret"
    };
    ctx.get("oauth_applications", &id)
        .ok_or_else(|| format!("Failed {action}; Linear error details were omitted because the response may contain a one-time secret"))?;
    if webhook {
        let secret = format!(
            "lin_wh_{}",
            ctx.st.ids.next_prefixed("oauth_wh_secret", "", 32)
        );
        Ok(json!({"success": true, "id": id, "webhookSecret": secret}))
    } else {
        let secret = format!(
            "lin_oauth_{}",
            ctx.st
                .ids
                .next_prefixed("oauth_secret", "", 32)
                .to_lowercase()
        );
        Ok(json!({"success": true, "id": id, "clientSecret": secret}))
    }
}

// ───────────────────────────── server ─────────────────────────────

fn rate_limit() -> Value {
    json!({"blockedUntil": 0, "isBlocked": false, "lastRateLimitedAt": null, "lastDelayMs": null, "lastOperation": null})
}

pub fn rate_limit_status(_ctx: &mut Ctx, _args: &Value) -> R {
    Ok(rate_limit())
}

pub fn server_status(_ctx: &mut Ctx, _args: &Value) -> R {
    Ok(json!({
        "version": "1.4.5", "pid": 1, "uptimeSeconds": 0, "toolCount": 198, "resourceCount": 6,
        "promptCount": 4, "rateLimit": rate_limit(),
    }))
}
