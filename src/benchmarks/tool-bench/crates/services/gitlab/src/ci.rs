//! CI/CD: pipelines, jobs (traces, artifacts, retry/cancel/play/erase), triggers, schedules,
//! CI lint, CI variables, deployments, environments, runners and commit statuses.
//!
//! Collections (all id-keyed unless noted; `project` = project path):
//! - `pipelines`: `id, iid, project, ref, sha, status, source, created_at, updated_at,
//!   started_at, finished_at, duration, queued_duration, coverage, user, merge_request, tag,
//!   variables, name, test_report, progress` (`progress` = statuses the pipeline steps through on
//!   subsequent reads; it lets new pipelines finish deterministically).
//! - `jobs`: `id, project, pipeline_id, name, stage, status, ref, tag, created_at, started_at,
//!   finished_at, duration, queued_duration, failure_reason, allow_failure, when, user,
//!   runner_id, retried, trace, artifacts, artifact_files, kind ("bridge"), downstream_pipeline_id,
//!   retry_result, play_result, progress`.
//! - `pipeline_triggers`, `pipeline_schedules` (with `variables`), `deployments`,
//!   `environments`, `runners` (`projects`/`groups` paths), `commit_statuses`.
//! - `variables`: key `<project-or-group path>:<KEY>` (`:<scope>` appended when the
//!   environment scope is not `*`).

use axum::{
    Router,
    http::{HeaderValue, StatusCode, header},
    response::Response,
    routing::{get as rget, post, put},
};
use mock_core::{
    store::Store,
    world::{SharedWorld, WorldState},
};
use serde_json::{Value, json};

use crate::common::*;

pub fn routes() -> Router<SharedWorld> {
    let p = "/projects/{id}/pipelines/{pipeline_id}";
    let j = "/projects/{id}/jobs/{job_id}";
    let s = "/projects/{id}/pipeline_schedules/{schedule_id}";
    Router::new()
        .route("/projects/{id}/pipelines", rget(wrap(list_pipelines)))
        .route(
            "/projects/{id}/pipelines/latest",
            rget(wrap(latest_pipeline)),
        )
        .route("/projects/{id}/pipeline", post(wrap(create_pipeline)))
        .route(p, rget(wrap(get_pipeline)).delete(wrap(delete_pipeline)))
        .route(&format!("{p}/variables"), rget(wrap(pipeline_variables)))
        .route(&format!("{p}/test_report"), rget(wrap(test_report)))
        .route(
            &format!("{p}/test_report_summary"),
            rget(wrap(test_report_summary)),
        )
        .route(&format!("{p}/metadata"), put(wrap(pipeline_metadata)))
        .route(&format!("{p}/retry"), post(wrap(retry_pipeline)))
        .route(&format!("{p}/cancel"), post(wrap(cancel_pipeline)))
        .route(&format!("{p}/jobs"), rget(wrap(pipeline_jobs)))
        .route(&format!("{p}/bridges"), rget(wrap(pipeline_bridges)))
        .route(
            "/projects/{id}/trigger/pipeline",
            post(wrap(trigger_pipeline)),
        )
        .route("/projects/{id}/jobs", rget(wrap(project_jobs)))
        .route(j, rget(wrap(get_job)))
        .route(&format!("{j}/trace"), rget(wrap(job_trace)))
        .route(&format!("{j}/retry"), post(wrap(retry_job)))
        .route(&format!("{j}/cancel"), post(wrap(cancel_job)))
        .route(&format!("{j}/play"), post(wrap(play_job)))
        .route(&format!("{j}/erase"), post(wrap(erase_job)))
        .route(
            &format!("{j}/artifacts"),
            rget(wrap(job_artifacts_archive)).delete(wrap(delete_job_artifacts)),
        )
        .route(
            &format!("{j}/artifacts/{{*artifact_path}}"),
            rget(wrap(job_artifact_file)),
        )
        .route(
            "/projects/{id}/triggers",
            rget(wrap(list_triggers)).post(wrap(create_trigger)),
        )
        .route(
            "/projects/{id}/triggers/{trigger_id}",
            rget(wrap(get_trigger))
                .put(wrap(update_trigger))
                .delete(wrap(delete_trigger)),
        )
        .route(
            "/projects/{id}/pipeline_schedules",
            rget(wrap(list_schedules)).post(wrap(create_schedule)),
        )
        .route(
            s,
            rget(wrap(get_schedule))
                .put(wrap(update_schedule))
                .delete(wrap(delete_schedule)),
        )
        .route(&format!("{s}/pipelines"), rget(wrap(schedule_pipelines)))
        .route(&format!("{s}/play"), post(wrap(play_schedule)))
        .route(&format!("{s}/take_ownership"), post(wrap(take_ownership)))
        .route(
            &format!("{s}/variables"),
            post(wrap(create_schedule_variable)),
        )
        .route(
            &format!("{s}/variables/{{key}}"),
            rget(wrap(get_schedule_variable))
                .put(wrap(update_schedule_variable))
                .delete(wrap(delete_schedule_variable)),
        )
        .route(
            "/projects/{id}/ci/lint",
            rget(wrap(project_lint)).post(wrap(lint)),
        )
        .route("/ci/lint", post(wrap(lint)))
        .route(
            "/projects/{id}/variables",
            rget(wrap(list_variables)).post(wrap(create_variable)),
        )
        .route(
            "/projects/{id}/variables/{key}",
            rget(wrap(get_variable))
                .put(wrap(update_variable))
                .delete(wrap(delete_variable)),
        )
        .route(
            "/groups/{id}/variables",
            rget(wrap(list_variables)).post(wrap(create_variable)),
        )
        .route(
            "/groups/{id}/variables/{key}",
            rget(wrap(get_variable))
                .put(wrap(update_variable))
                .delete(wrap(delete_variable)),
        )
        .route(
            "/projects/{id}/deployments",
            rget(wrap(list_deployments)).post(wrap(create_deployment)),
        )
        .route(
            "/projects/{id}/deployments/{deployment_id}",
            rget(wrap(get_deployment))
                .put(wrap(update_deployment))
                .delete(wrap(delete_deployment)),
        )
        .route(
            "/projects/{id}/deployments/{deployment_id}/merge_requests",
            rget(wrap(deployment_mrs)),
        )
        .route(
            "/projects/{id}/deployments/{deployment_id}/approval",
            post(wrap(approve_deployment)),
        )
        .route(
            "/projects/{id}/environments",
            rget(wrap(list_environments)).post(wrap(create_environment)),
        )
        .route(
            "/projects/{id}/environments/stop_stale",
            post(wrap(stop_stale)),
        )
        .route(
            "/projects/{id}/environments/review_apps",
            axum::routing::delete(wrap(delete_review_apps)),
        )
        .route(
            "/projects/{id}/environments/{environment_id}",
            rget(wrap(get_environment))
                .put(wrap(update_environment))
                .delete(wrap(delete_environment)),
        )
        .route(
            "/projects/{id}/environments/{environment_id}/stop",
            post(wrap(stop_environment)),
        )
        .route("/projects/{id}/runners", rget(wrap(project_runners)))
        .route("/runners", rget(wrap(owned_runners)))
        .route("/runners/{runner_id}", rget(wrap(get_runner)))
        .route(
            "/projects/{id}/repository/commits/{sha}/statuses",
            rget(wrap(list_statuses)),
        )
        .route("/projects/{id}/statuses/{sha}", post(wrap(create_status)))
}

const ACTIVE: &[&str] = &[
    "created",
    "waiting_for_resource",
    "preparing",
    "pending",
    "running",
    "scheduled",
];

fn is_active(status: &str) -> bool {
    ACTIVE.contains(&status)
}

fn proj_of(doc: &Value) -> &str {
    str_of(doc, "project")
}

// ---------------------------------------------------------------------------
// Pipeline rendering and lookups

fn pipeline_web_url(base: &str, doc: &Value) -> String {
    format!("{base}/{}/-/pipelines/{}", proj_of(doc), doc["id"])
}

pub(crate) fn render_pipeline_basic(st: &Store, base: &str, doc: &Value) -> Value {
    json!({
        "id": doc["id"],
        "iid": doc.get("iid").cloned().unwrap_or_else(|| doc["id"].clone()),
        "project_id": project_id(st, proj_of(doc)),
        "sha": str_of(doc, "sha"),
        "ref": str_of(doc, "ref"),
        "status": str_of(doc, "status"),
        "source": doc.get("source").cloned().unwrap_or(json!("push")),
        "created_at": doc["created_at"],
        "updated_at": doc.get("updated_at").unwrap_or(&doc["created_at"]),
        "web_url": pipeline_web_url(base, doc),
        "name": doc.get("name").cloned().unwrap_or(Value::Null),
    })
}

fn detailed_status(base: &str, doc: &Value, status: &str) -> Value {
    let (icon, text, label, group) = match status {
        "success" => ("status_success", "Passed", "passed", "success"),
        "failed" => ("status_failed", "Failed", "failed", "failed"),
        "running" => ("status_running", "Running", "running", "running"),
        "pending" => ("status_pending", "Pending", "pending", "pending"),
        "canceled" => ("status_canceled", "Canceled", "canceled", "canceled"),
        "skipped" => ("status_skipped", "Skipped", "skipped", "skipped"),
        "manual" => ("status_manual", "Manual", "manual action", "manual"),
        "created" => ("status_created", "Created", "created", "created"),
        "scheduled" => ("status_scheduled", "Scheduled", "scheduled", "scheduled"),
        _ => ("status_notfound", status, status, status),
    };
    let path = format!("/{}/-/pipelines/{}", proj_of(doc), doc["id"]);
    let _ = base;
    json!({
        "icon": icon,
        "text": text,
        "label": label,
        "group": group,
        "tooltip": label,
        "has_details": true,
        "details_path": path,
        "illustration": null,
        "favicon": format!("/assets/ci_favicons/favicon_{icon}.png"),
    })
}

pub(crate) fn render_pipeline(st: &Store, base: &str, doc: &Value) -> Value {
    let mut out = render_pipeline_basic(st, base, doc);
    let status = str_of(doc, "status").to_owned();
    let project = proj_of(doc).to_owned();
    let committed = crate::repository::commit_doc(st, &project, str_of(doc, "sha"))
        .map(|c| c["committed_date"].clone())
        .unwrap_or(Value::Null);
    merge_into(
        &mut out,
        json!({
            "before_sha": doc.get("before_sha").cloned().unwrap_or(json!("0000000000000000000000000000000000000000")),
            "tag": doc.get("tag").cloned().unwrap_or(json!(false)),
            "yaml_errors": doc.get("yaml_errors").cloned().unwrap_or(Value::Null),
            "user": user_opt(st, base, &doc["user"]),
            "started_at": doc.get("started_at").cloned().unwrap_or(Value::Null),
            "finished_at": doc.get("finished_at").cloned().unwrap_or(Value::Null),
            "committed_at": committed,
            "duration": doc.get("duration").cloned().unwrap_or(Value::Null),
            "queued_duration": doc.get("queued_duration").cloned().unwrap_or(Value::Null),
            "coverage": doc.get("coverage").cloned().unwrap_or(Value::Null),
            "detailed_status": detailed_status(base, doc, &status),
            "archived": false,
        }),
    );
    if out["user"].is_null()
        && let Some(o) = out.as_object_mut()
    {
        o.remove("user");
    }
    out
}

fn pipelines_of(st: &Store, project: &str) -> Vec<Value> {
    let mut out: Vec<Value> = all(st, "pipelines")
        .into_iter()
        .filter(|(_, p)| proj_of(p) == project)
        .map(|(_, p)| p.clone())
        .collect();
    out.sort_by(|a, b| as_u64(&b["id"]).cmp(&as_u64(&a["id"])));
    out
}

/// Pipelines of an MR, newest first: explicit `merge_request` links, else source-branch pipelines.
pub(crate) fn mr_pipelines(st: &Store, key: &str) -> Vec<Value> {
    let Some(m) = get(st, "merge_requests", key) else {
        return Vec::new();
    };
    let project = m
        .get("source_project")
        .and_then(Value::as_str)
        .unwrap_or_else(|| project_of(key))
        .to_owned();
    let branch = str_of(m, "source_branch").to_owned();
    let mut out: Vec<Value> = all(st, "pipelines")
        .into_iter()
        .filter(|(_, p)| {
            str_of(p, "merge_request") == key
                || (p.get("merge_request").is_none_or(Value::is_null)
                    && proj_of(p) == project
                    && str_of(p, "ref") == branch)
        })
        .map(|(_, p)| p.clone())
        .collect();
    out.sort_by(|a, b| as_u64(&b["id"]).cmp(&as_u64(&a["id"])));
    out
}

pub(crate) fn latest_mr_pipeline(st: &Store, key: &str) -> Option<Value> {
    mr_pipelines(st, key).into_iter().next()
}

pub(crate) fn latest_pipeline_for_sha(st: &Store, project: &str, sha: &str) -> Option<Value> {
    pipelines_of(st, project)
        .into_iter()
        .find(|p| str_of(p, "sha") == sha)
}

fn require_pipeline(st: &Store, r: &Req) -> Result<(String, String), Response> {
    let project = require_project(st, r)?;
    let id = r.p("pipeline_id").to_owned();
    match get(st, "pipelines", &id) {
        Some(p) if proj_of(p) == project => Ok((project, id)),
        _ => Err(not_found("Pipeline")),
    }
}

fn jobs_of_pipeline(st: &Store, pipeline_id: &str) -> Vec<Value> {
    all(st, "jobs")
        .into_iter()
        .filter(|(_, j)| id_is(&j["pipeline_id"], pipeline_id))
        .map(|(_, j)| j.clone())
        .collect()
}

/// Steps a pipeline (and its jobs) one status forward if it has queued `progress`.
fn advance(w: &mut WorldState, pipeline_id: &str) {
    let has = store(w)
        .collection("pipelines")
        .and_then(|c| c.get(pipeline_id))
        .and_then(|p| p.get("progress")?.as_array().map(|a| !a.is_empty()))
        .unwrap_or(false);
    let job_ids: Vec<String> = all(store(w), "jobs")
        .iter()
        .filter(|(_, j)| {
            id_is(&j["pipeline_id"], pipeline_id)
                && j.get("progress")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
        })
        .map(|(k, _)| (*k).to_owned())
        .collect();
    if !has && job_ids.is_empty() {
        return;
    }
    let now = now(w);
    let st = svc_mut(w);
    for id in job_ids {
        step_job(st, &id, &now);
    }
    if let Some(p) = st.collection_mut("pipelines").get_mut(pipeline_id)
        && let Some(next) = p
            .get_mut("progress")
            .and_then(Value::as_array_mut)
            .filter(|a| !a.is_empty())
            .map(|a| a.remove(0))
    {
        let status = next.as_str().unwrap_or("success").to_owned();
        if status == "running" && p["started_at"].is_null() {
            p["started_at"] = json!(now);
        }
        if !is_active(&status) {
            p["finished_at"] = json!(now);
            p["duration"] = json!(42);
        }
        p["status"] = json!(status);
        p["updated_at"] = json!(now);
    }
}

fn step_job(st: &mut Store, id: &str, now: &str) {
    let Some(j) = st.collection_mut("jobs").get_mut(id) else {
        return;
    };
    let Some(next) = j
        .get_mut("progress")
        .and_then(Value::as_array_mut)
        .filter(|a| !a.is_empty())
        .map(|a| a.remove(0))
    else {
        return;
    };
    let status = next.as_str().unwrap_or("success").to_owned();
    if status == "running" {
        j["started_at"] = json!(now);
    }
    if !is_active(&status) {
        if j["started_at"].is_null() {
            j["started_at"] = json!(now);
        }
        j["finished_at"] = json!(now);
        j["duration"] = json!(21.4);
        if j.get("trace").is_none_or(Value::is_null) {
            let name = str_of(j, "name").to_owned();
            let script = j
                .get("script")
                .and_then(Value::as_str)
                .unwrap_or("echo done")
                .to_owned();
            let tail = if status == "success" {
                "Job succeeded".to_owned()
            } else {
                "ERROR: Job failed: exit code 1".to_owned()
            };
            j["trace"] = json!(format!(
                "Running with gitlab-runner 17.11.0 (8ec04662)\n  on shared-runner-1 xk2B9a, system ID: s_0e6850b2bce1\nPreparing the \"docker\" executor\nGetting source from Git repository\nExecuting \"step_script\" stage of the job script\n$ {script}\n{name}: done\nCleaning up project directory and file based variables\n{tail}\n"
            ));
        }
        if status == "failed" && j["failure_reason"].is_null() {
            j["failure_reason"] = json!("script_failure");
        }
    }
    j["status"] = json!(status);
}

fn advance_job(w: &mut WorldState, job_id: &str) {
    let pipeline = store(w)
        .collection("jobs")
        .and_then(|c| c.get(job_id))
        .map(|j| j["pipeline_id"].to_string());
    let now = now(w);
    step_job(svc_mut(w), job_id, &now);
    if let Some(p) = pipeline {
        let has = store(w)
            .collection("pipelines")
            .and_then(|c| c.get(&p))
            .and_then(|p| p.get("progress")?.as_array().map(|a| !a.is_empty()))
            .unwrap_or(false);
        if has {
            advance(w, &p);
        }
    }
}

// ---------------------------------------------------------------------------
// Pipelines

fn list_pipelines(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut items: Vec<Value> = pipelines_of(st, &project)
        .into_iter()
        .filter(|p| {
            let status = str_of(p, "status");
            r.q("status").is_none_or(|s| status == s)
                && r.q("ref").is_none_or(|x| str_of(p, "ref") == x)
                && r.q("sha").is_none_or(|x| str_of(p, "sha") == x)
                && r.q("source")
                    .is_none_or(|x| p.get("source").and_then(Value::as_str).unwrap_or("push") == x)
                && r.q("name").is_none_or(|x| str_of(p, "name") == x)
                && r.q("username").is_none_or(|x| str_of(p, "user") == x)
                && r.q("updated_after")
                    .is_none_or(|x| str_of(p, "updated_at") >= x)
                && r.q("updated_before")
                    .is_none_or(|x| str_of(p, "updated_at") <= x)
                && r.q("created_after")
                    .is_none_or(|x| str_of(p, "created_at") >= x)
                && r.q("created_before")
                    .is_none_or(|x| str_of(p, "created_at") <= x)
                && (r.q("yaml_errors") != Some("true") || !p["yaml_errors"].is_null())
                && match r.q("scope") {
                    Some("running") => status == "running",
                    Some("pending") => status == "pending",
                    Some("finished") => !is_active(status),
                    Some("branches") => !p["tag"].as_bool().unwrap_or(false),
                    Some("tags") => p["tag"].as_bool().unwrap_or(false),
                    _ => true,
                }
        })
        .collect();
    let asc = r.q("sort") == Some("asc");
    let order = r.q("order_by").unwrap_or("id");
    items.sort_by(|a, b| {
        let ord = match order {
            "updated_at" => str_of(a, "updated_at").cmp(str_of(b, "updated_at")),
            "status" => str_of(a, "status").cmp(str_of(b, "status")),
            "ref" => str_of(a, "ref").cmp(str_of(b, "ref")),
            _ => std::cmp::Ordering::Equal,
        }
        .then(as_u64(&a["id"]).cmp(&as_u64(&b["id"])));
        if asc { ord } else { ord.reverse() }
    });
    let items = items
        .iter()
        .map(|p| render_pipeline_basic(st, &r.base, p))
        .collect();
    paged(r, items)
}

fn latest_pipeline(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let reference = r
        .q("ref")
        .map(str::to_owned)
        .unwrap_or_else(|| crate::repository::default_branch(st, &project));
    match pipelines_of(st, &project)
        .into_iter()
        .find(|p| str_of(p, "ref") == reference)
    {
        Some(p) => ok(render_pipeline(st, &r.base, &p)),
        None => not_found("Pipeline"),
    }
}

fn get_pipeline(w: &mut WorldState, r: &Req) -> Response {
    let (_, id) = match require_pipeline(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    advance(w, &id);
    let st = store(w);
    let doc = get(st, "pipelines", &id).cloned().unwrap_or(json!({}));
    ok(render_pipeline(st, &r.base, &doc))
}

fn delete_pipeline(w: &mut WorldState, r: &Req) -> Response {
    let (_, id) = match require_pipeline(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    remove_doc(st, "pipelines", &id);
    let jobs: Vec<String> = all(st, "jobs")
        .iter()
        .filter(|(_, j)| id_is(&j["pipeline_id"], &id))
        .map(|(k, _)| (*k).to_owned())
        .collect();
    for j in jobs {
        remove_doc(st, "jobs", &j);
    }
    no_content()
}

fn pipeline_variables(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (_, id) = match require_pipeline(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let vars: Vec<Value> = get(st, "pipelines", &id)
        .and_then(|p| p.get("variables")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|v| {
            json!({
                "variable_type": v.get("variable_type").cloned().unwrap_or(json!("env_var")),
                "key": str_of(v, "key"),
                "value": v.get("value").cloned().unwrap_or(json!("")),
                "raw": v.get("raw").cloned().unwrap_or(json!(false)),
            })
        })
        .collect();
    ok(Value::Array(vars))
}

/// Without JUnit artifacts GitLab reports an empty test report.
fn default_report(_st: &Store, _id: &str) -> Value {
    json!({"total_time": 0.0, "total_count": 0, "success_count": 0, "failed_count": 0, "skipped_count": 0, "error_count": 0, "test_suites": []})
}

fn test_report(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (_, id) = match require_pipeline(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let report = get(st, "pipelines", &id)
        .and_then(|p| p.get("test_report").cloned())
        .unwrap_or_else(|| default_report(st, &id));
    ok(report)
}

fn test_report_summary(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (_, id) = match require_pipeline(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let report = get(st, "pipelines", &id)
        .and_then(|p| p.get("test_report").cloned())
        .unwrap_or_else(|| default_report(st, &id));
    let job_ids: Vec<u64> = jobs_of_pipeline(st, &id)
        .iter()
        .filter(|j| str_of(j, "stage") == "test")
        .filter_map(|j| as_u64(&j["id"]))
        .collect();
    let suites: Vec<Value> = report["test_suites"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| {
            json!({
                "name": s["name"], "total_time": s["total_time"], "total_count": s["total_count"],
                "success_count": s["success_count"], "failed_count": s["failed_count"],
                "skipped_count": s["skipped_count"], "error_count": s["error_count"],
                "build_ids": job_ids, "suite_error": s.get("suite_error").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    ok(json!({
        "total": {
            "time": report["total_time"], "count": report["total_count"], "success": report["success_count"],
            "failed": report["failed_count"], "skipped": report["skipped_count"], "error": report["error_count"],
            "suite_error": null,
        },
        "test_suites": suites,
    }))
}

fn pipeline_metadata(w: &mut WorldState, r: &Req) -> Response {
    let (_, id) = match require_pipeline(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Some(name) = r.s("name") else {
        return missing("name");
    };
    let now = now(w);
    let st = svc_mut(w);
    let p = st
        .collection_mut("pipelines")
        .get_mut(&id)
        .expect("pipeline");
    p["name"] = json!(name);
    p["updated_at"] = json!(now);
    let p = p.clone();
    ok(render_pipeline(st, &r.base, &p))
}

fn final_status(jobs: &[Value]) -> String {
    let current: Vec<&Value> = jobs
        .iter()
        .filter(|j| !j["retried"].as_bool().unwrap_or(false))
        .collect();
    let outcome = |j: &Value| -> String {
        // The status a job ends in once its queued progress has played out.
        j.get("progress")
            .and_then(Value::as_array)
            .and_then(|a| a.last())
            .and_then(Value::as_str)
            .unwrap_or_else(|| str_of(j, "status"))
            .to_owned()
    };
    if current
        .iter()
        .any(|j| outcome(j) == "failed" && !j["allow_failure"].as_bool().unwrap_or(false))
    {
        "failed".into()
    } else if current.iter().any(|j| outcome(j) == "canceled") {
        "canceled".into()
    } else if !current.is_empty()
        && current
            .iter()
            .all(|j| outcome(j) == "manual" || outcome(j) == "skipped")
    {
        "manual".into()
    } else {
        "success".into()
    }
}

fn retry_pipeline(w: &mut WorldState, r: &Req) -> Response {
    let (_, id) = match require_pipeline(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let jobs = jobs_of_pipeline(store(w), &id);
    let now = now(w);
    let st = svc_mut(w);
    for j in jobs.iter().filter(|j| {
        !j["retried"].as_bool().unwrap_or(false)
            && ["failed", "canceled"].contains(&str_of(j, "status"))
    }) {
        clone_retry(st, j, &now);
    }
    let all_jobs = jobs_of_pipeline(st, &id);
    let fin = final_status(&all_jobs);
    let p = st
        .collection_mut("pipelines")
        .get_mut(&id)
        .expect("pipeline");
    p["status"] = json!("running");
    p["finished_at"] = Value::Null;
    p["updated_at"] = json!(now);
    p["progress"] = json!([fin]);
    let p = p.clone();
    created(render_pipeline(st, &r.base, &p))
}

/// Marks `j` retried and inserts a fresh copy that will finish as `retry_result`.
fn clone_retry(st: &mut Store, j: &Value, now: &str) -> Value {
    let old_id = j["id"].to_string();
    let new_id = next_id(st, "jobs", 1);
    if let Some(o) = st.collection_mut("jobs").get_mut(&old_id) {
        o["retried"] = json!(true);
    }
    let result = j
        .get("retry_result")
        .and_then(Value::as_str)
        .unwrap_or("success")
        .to_owned();
    let mut n = j.clone();
    n["id"] = json!(new_id);
    n["status"] = json!("pending");
    n["created_at"] = json!(now);
    n["started_at"] = Value::Null;
    n["finished_at"] = Value::Null;
    n["duration"] = Value::Null;
    n["failure_reason"] = Value::Null;
    n["retried"] = json!(false);
    n["trace"] = j.get("retry_trace").cloned().unwrap_or(Value::Null);
    n["progress"] = json!([result]);
    n["user"] = json!(current_username(st));
    if let Some(o) = n.as_object_mut() {
        o.remove("retry_result");
        o.remove("retry_trace");
    }
    st.collection_mut("jobs")
        .insert(new_id.to_string(), n.clone());
    n
}

fn cancel_pipeline(w: &mut WorldState, r: &Req) -> Response {
    let (_, id) = match require_pipeline(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let jobs = jobs_of_pipeline(store(w), &id);
    let now = now(w);
    let st = svc_mut(w);
    for j in jobs
        .iter()
        .filter(|j| is_active(str_of(j, "status")) || str_of(j, "status") == "manual")
    {
        if let Some(d) = st.collection_mut("jobs").get_mut(&j["id"].to_string())
            && str_of(d, "status") != "manual"
        {
            d["status"] = json!("canceled");
            d["finished_at"] = json!(now);
            d["progress"] = json!([]);
        }
    }
    let p = st
        .collection_mut("pipelines")
        .get_mut(&id)
        .expect("pipeline");
    if is_active(str_of(p, "status")) {
        p["status"] = json!("canceled");
        p["finished_at"] = json!(now);
    }
    p["progress"] = json!([]);
    p["updated_at"] = json!(now);
    let p = p.clone();
    ok(render_pipeline(st, &r.base, &p))
}

fn create_pipeline(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(reference) = r.s("ref") else {
        return missing("ref");
    };
    let vars = r.body.get("variables").cloned().unwrap_or(json!([]));
    match start_pipeline(w, &project, &reference, "api", vars, None) {
        Ok(p) => created(render_pipeline(store(w), &r.base, &p)),
        Err(e) => e,
    }
}

/// Creates a pipeline (and its jobs from `.gitlab-ci.yml`) for `reference`.
fn start_pipeline(
    w: &mut WorldState,
    project: &str,
    reference: &str,
    source: &str,
    variables: Value,
    merge_request: Option<&str>,
) -> Result<Value, Response> {
    let st = store(w);
    let Some(snap) = crate::repository::resolve_ref(st, project, reference) else {
        return Err(json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": {"base": ["Reference not found"]}}),
        ));
    };
    let ci_path = get(st, "projects", project)
        .and_then(|p| p.get("ci_config_path")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| ".gitlab-ci.yml".into());
    let Some(config) = crate::repository::file_at(st, project, &snap.branch, &ci_path) else {
        return Err(json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": {"base": ["Missing CI config file"]}}),
        ));
    };
    let parsed = parse_ci(&config);
    if !parsed.errors.is_empty() {
        return Err(json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": {"base": parsed.errors}}),
        ));
    }
    let results = get(st, "projects", project)
        .and_then(|p| p.get("ci_results").cloned())
        .unwrap_or(json!({}));
    let is_tag = get(st, "tags", &format!("{project}:{reference}")).is_some();
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "pipelines", 1);
    let iid = all(st, "pipelines")
        .iter()
        .filter(|(_, p)| proj_of(p) == project)
        .filter_map(|(_, p)| as_u64(&p["iid"]))
        .max()
        .unwrap_or(0)
        + 1;
    let user = current_username(st);
    let mut jobs = Vec::new();
    let first_stage = parsed
        .stages
        .iter()
        .find(|s| parsed.jobs.iter().any(|j| &j.stage == *s))
        .cloned();
    for job in &parsed.jobs {
        let jid = next_id(st, "jobs", 1) + jobs.len() as u64;
        let outcome = results
            .get(&job.name)
            .and_then(Value::as_str)
            .unwrap_or("success")
            .to_owned();
        let initial = if job.manual {
            "manual"
        } else if Some(&job.stage) == first_stage.as_ref() {
            "pending"
        } else {
            "created"
        };
        let progress = if job.manual {
            json!([])
        } else {
            json!(["running", outcome])
        };
        jobs.push(json!({
            "id": jid,
            "project": project,
            "pipeline_id": id,
            "name": job.name,
            "stage": job.stage,
            "status": initial,
            "ref": reference,
            "tag": is_tag,
            "created_at": now,
            "started_at": null,
            "finished_at": null,
            "duration": null,
            "queued_duration": null,
            "failure_reason": null,
            "allow_failure": job.allow_failure,
            "when": if job.manual { "manual" } else { "on_success" },
            "user": user,
            "runner_id": null,
            "retried": false,
            "script": job.script,
            "progress": progress,
        }));
    }
    for j in &jobs {
        st.collection_mut("jobs")
            .insert(j["id"].to_string(), j.clone());
    }
    let fin = final_status(&jobs);
    let doc = json!({
        "id": id,
        "iid": iid,
        "project": project,
        "ref": reference,
        "sha": snap.sha,
        "status": "created",
        "source": source,
        "created_at": now,
        "updated_at": now,
        "started_at": null,
        "finished_at": null,
        "duration": null,
        "queued_duration": null,
        "coverage": null,
        "user": user,
        "merge_request": merge_request,
        "tag": is_tag,
        "variables": variables,
        "progress": ["running", fin],
    });
    st.collection_mut("pipelines")
        .insert(id.to_string(), doc.clone());
    Ok(doc)
}

pub(crate) fn create_mr_pipeline(w: &mut WorldState, r: &Req, key: &str) -> Response {
    let st = store(w);
    let m = get(st, "merge_requests", key).cloned().unwrap_or(json!({}));
    let project = project_of(key).to_owned();
    let branch = str_of(&m, "source_branch").to_owned();
    match start_pipeline(
        w,
        &project,
        &branch,
        "merge_request_event",
        json!([]),
        Some(key),
    ) {
        Ok(p) => {
            let st = store(w);
            let mut out = render_pipeline(st, &r.base, &p);
            out["ref"] = json!(format!("refs/merge-requests/{}/head", iid_of(key)));
            created(out)
        }
        Err(e) => e,
    }
}

fn trigger_pipeline(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(token) = r.s("token") else {
        return missing("token");
    };
    let Some(reference) = r.s("ref") else {
        return missing("ref");
    };
    let valid = all(st, "pipeline_triggers")
        .iter()
        .any(|(_, t)| proj_of(t) == project && str_of(t, "token") == token);
    if !valid {
        return error(StatusCode::NOT_FOUND, "404 Not Found");
    }
    let vars: Vec<Value> = r
        .body
        .get("variables")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| json!({"key": k, "value": v, "variable_type": "env_var"}))
                .collect()
        })
        .unwrap_or_default();
    match start_pipeline(w, &project, &reference, "trigger", Value::Array(vars), None) {
        Ok(p) => created(render_pipeline(store(w), &r.base, &p)),
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------
// Jobs

fn render_job(st: &Store, base: &str, j: &Value) -> Value {
    let project = proj_of(j).to_owned();
    let pid = j["pipeline_id"].to_string();
    let pipeline = get(st, "pipelines", &pid).cloned();
    let sha = pipeline
        .as_ref()
        .map(|p| str_of(p, "sha").to_owned())
        .unwrap_or_default();
    let user = j
        .get("user")
        .and_then(Value::as_str)
        .map(|u| crate::projects::user_self(st, base, u));
    let runner =
        as_u64(&j["runner_id"]).and_then(|rid| get(st, "runners", &rid.to_string()).cloned());
    let artifacts = j.get("artifacts").cloned().unwrap_or(json!([]));
    let mut out = json!({
        "id": j["id"],
        "status": str_of(j, "status"),
        "stage": j.get("stage").and_then(Value::as_str).unwrap_or("test"),
        "name": str_of(j, "name"),
        "ref": j.get("ref").and_then(Value::as_str).unwrap_or_else(|| pipeline.as_ref().map_or("", |p| str_of(p, "ref"))),
        "tag": j.get("tag").cloned().unwrap_or(json!(false)),
        "coverage": j.get("coverage").cloned().unwrap_or(Value::Null),
        "allow_failure": j.get("allow_failure").cloned().unwrap_or(json!(false)),
        "created_at": j["created_at"],
        "started_at": j.get("started_at").cloned().unwrap_or(Value::Null),
        "finished_at": j.get("finished_at").cloned().unwrap_or(Value::Null),
        "erased_at": j.get("erased_at").cloned().unwrap_or(Value::Null),
        "duration": j.get("duration").cloned().unwrap_or(Value::Null),
        "queued_duration": j.get("queued_duration").cloned().unwrap_or(Value::Null),
        "commit": if sha.is_empty() { Value::Null } else { crate::repository::render_commit(st, base, &project, &sha) },
        "pipeline": pipeline.as_ref().map(|p| render_pipeline_basic(st, base, p)).unwrap_or(Value::Null),
        "failure_reason": j.get("failure_reason").cloned().unwrap_or(Value::Null),
        "web_url": format!("{base}/{project}/-/jobs/{}", j["id"]),
        "project": {"ci_job_token_scope_enabled": false},
        "artifacts": artifacts,
        "artifacts_expire_at": null,
        "archived": false,
        "retried": j.get("retried").cloned().unwrap_or(json!(false)),
        "when": j.get("when").cloned().unwrap_or(json!("on_success")),
        "tag_list": j.get("tag_list").cloned().unwrap_or(json!([])),
        "runner": runner.map(|r| json!({
            "id": r["id"], "description": str_of(&r, "description"), "ip_address": null,
            "active": !r["paused"].as_bool().unwrap_or(false), "paused": r.get("paused").cloned().unwrap_or(json!(false)),
            "is_shared": r.get("is_shared").cloned().unwrap_or(json!(true)), "runner_type": r.get("runner_type").cloned().unwrap_or(json!("instance_type")),
            "name": r.get("name").cloned().unwrap_or(json!("gitlab-runner")), "online": true, "status": "online",
        })).unwrap_or(Value::Null),
        "runner_manager": null,
    });
    if let Some(u) = user {
        out["user"] = u;
    }
    if out["commit"].is_null()
        && let Some(o) = out.as_object_mut()
    {
        o.remove("commit");
    }
    if out["pipeline"].is_null()
        && let Some(o) = out.as_object_mut()
    {
        o.remove("pipeline");
    }
    if out["failure_reason"].is_null()
        && let Some(o) = out.as_object_mut()
    {
        o.remove("failure_reason");
    }
    if let Some(a) = artifacts
        .as_array()
        .and_then(|a| a.iter().find(|a| str_of(a, "file_type") == "archive"))
    {
        out["artifacts_file"] = json!({"filename": a["filename"], "size": a["size"]});
    }
    if str_of(j, "kind") == "bridge" {
        let down = as_u64(&j["downstream_pipeline_id"])
            .and_then(|d| get(st, "pipelines", &d.to_string()).cloned())
            .map(|p| render_pipeline_basic(st, base, &p))
            .unwrap_or(Value::Null);
        out["downstream_pipeline"] = down;
    }
    out
}

fn filter_jobs(r: &Req, jobs: Vec<Value>) -> Vec<Value> {
    let scopes = r.list("scope").unwrap_or_default();
    let include_retried = r.q("include_retried") == Some("true");
    jobs.into_iter()
        .filter(|j| include_retried || !j["retried"].as_bool().unwrap_or(false))
        .filter(|j| scopes.is_empty() || scopes.iter().any(|s| s == str_of(j, "status")))
        .collect()
}

fn pipeline_jobs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (_, id) = match require_pipeline(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let jobs: Vec<Value> = jobs_of_pipeline(st, &id)
        .into_iter()
        .filter(|j| str_of(j, "kind") != "bridge")
        .collect();
    let items = filter_jobs(r, jobs)
        .iter()
        .map(|j| render_job(st, &r.base, j))
        .collect();
    paged(r, items)
}

fn pipeline_bridges(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (_, id) = match require_pipeline(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let jobs: Vec<Value> = jobs_of_pipeline(st, &id)
        .into_iter()
        .filter(|j| str_of(j, "kind") == "bridge")
        .collect();
    let items = filter_jobs(r, jobs)
        .iter()
        .map(|j| render_job(st, &r.base, j))
        .collect();
    paged(r, items)
}

fn project_jobs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut jobs: Vec<Value> = all(st, "jobs")
        .into_iter()
        .filter(|(_, j)| proj_of(j) == project && str_of(j, "kind") != "bridge")
        .map(|(_, j)| j.clone())
        .collect();
    jobs.sort_by(|a, b| as_u64(&b["id"]).cmp(&as_u64(&a["id"])));
    let mut q = r.clone();
    q.query.push(("include_retried".into(), "true".into()));
    let items = filter_jobs(&q, jobs)
        .iter()
        .map(|j| render_job(st, &r.base, j))
        .collect();
    paged(r, items)
}

fn require_job(st: &Store, r: &Req) -> Result<String, Response> {
    let project = require_project(st, r)?;
    let id = r.p("job_id").to_owned();
    match get(st, "jobs", &id) {
        Some(j) if proj_of(j) == project => Ok(id),
        _ => Err(not_found("Job")),
    }
}

fn get_job(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_job(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    advance_job(w, &id);
    let st = store(w);
    let j = get(st, "jobs", &id).cloned().unwrap_or(json!({}));
    ok(render_job(st, &r.base, &j))
}

fn job_trace(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let id = match require_job(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let j = get(st, "jobs", &id).cloned().unwrap_or(json!({}));
    if j.get("erased_at").is_some_and(|e| !e.is_null()) {
        return text(StatusCode::OK, "text/plain; charset=utf-8", String::new());
    }
    let trace = j
        .get("trace")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if ["created", "pending", "manual", "skipped", "scheduled"]
                .contains(&str_of(&j, "status"))
            {
                String::new()
            } else {
                format!(
                    "Running with gitlab-runner 17.11.0 (8ec04662)\n$ {}\n",
                    str_of(&j, "name")
                )
            }
        });
    text(StatusCode::OK, "text/plain; charset=utf-8", trace)
}

fn retry_job(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_job(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let j = get(store(w), "jobs", &id).cloned().unwrap_or(json!({}));
    if is_active(str_of(&j, "status")) || str_of(&j, "status") == "manual" {
        return error(
            StatusCode::FORBIDDEN,
            "403 Forbidden - Job is not retryable",
        );
    }
    let now = now(w);
    let st = svc_mut(w);
    let n = clone_retry(st, &j, &now);
    let pid = j["pipeline_id"].to_string();
    let jobs = jobs_of_pipeline(st, &pid);
    let fin = final_status(&jobs);
    if let Some(p) = st.collection_mut("pipelines").get_mut(&pid) {
        p["status"] = json!("running");
        p["finished_at"] = Value::Null;
        p["updated_at"] = json!(now);
        p["progress"] = json!([fin]);
    }
    created(render_job(st, &r.base, &n))
}

fn cancel_job(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_job(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let now = now(w);
    let st = svc_mut(w);
    let j = st.collection_mut("jobs").get_mut(&id).expect("job");
    if is_active(str_of(j, "status")) {
        j["status"] = json!("canceled");
        j["finished_at"] = json!(now);
        j["progress"] = json!([]);
    }
    let j = j.clone();
    created(render_job(st, &r.base, &j))
}

fn play_job(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_job(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let j = get(store(w), "jobs", &id).cloned().unwrap_or(json!({}));
    if str_of(&j, "status") != "manual" && str_of(&j, "when") != "manual" {
        return bad_request("Unplayable Job");
    }
    if is_active(str_of(&j, "status")) {
        return bad_request("Unplayable Job");
    }
    let now = now(w);
    let st = svc_mut(w);
    let user = current_username(st);
    let result = j
        .get("play_result")
        .and_then(Value::as_str)
        .unwrap_or("success")
        .to_owned();
    let d = st.collection_mut("jobs").get_mut(&id).expect("job");
    d["status"] = json!("pending");
    d["user"] = json!(user);
    d["progress"] = json!([result]);
    d["started_at"] = Value::Null;
    d["finished_at"] = Value::Null;
    if let Some(vars) = r.body.get("job_variables_attributes") {
        d["variables"] = vars.clone();
    }
    let d = d.clone();
    let pid = j["pipeline_id"].to_string();
    let jobs = jobs_of_pipeline(st, &pid);
    let fin = final_status(&jobs);
    if let Some(p) = st.collection_mut("pipelines").get_mut(&pid) {
        p["status"] = json!("running");
        p["updated_at"] = json!(now);
        p["finished_at"] = Value::Null;
        p["progress"] = json!([fin]);
    }
    ok(render_job(st, &r.base, &d))
}

fn erase_job(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_job(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let now = now(w);
    let st = svc_mut(w);
    let j = st.collection_mut("jobs").get_mut(&id).expect("job");
    if is_active(str_of(j, "status")) {
        return forbidden();
    }
    j["erased_at"] = json!(now);
    j["trace"] = json!("");
    j["artifacts"] = json!([]);
    j["artifact_files"] = json!({});
    let j = j.clone();
    created(render_job(st, &r.base, &j))
}

/// A tiny stored-only ZIP archive of the job's artifact files.
fn zip_archive(files: &[(String, String)]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, content) in files {
        let offset = out.len() as u32;
        let crc = crc32(content.as_bytes());
        let size = content.len() as u32;
        let header = |sig: u32, central: bool| -> Vec<u8> {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes());
            }
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&0x5821u16.to_le_bytes());
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&size.to_le_bytes());
            h.extend_from_slice(&size.to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            if central {
                h.extend_from_slice(&0u16.to_le_bytes());
                h.extend_from_slice(&0u16.to_le_bytes());
                h.extend_from_slice(&0u16.to_le_bytes());
                h.extend_from_slice(&0u32.to_le_bytes());
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(content.as_bytes());
        central.extend(header(0x0201_4b50, true));
    }
    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend(central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn artifact_files(j: &Value) -> Vec<(String, String)> {
    j.get("artifact_files")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn job_artifacts_archive(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let id = match require_job(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let j = get(st, "jobs", &id).cloned().unwrap_or(json!({}));
    let files = artifact_files(&j);
    if files.is_empty() {
        return error(axum::http::StatusCode::NOT_FOUND, "404 Not found");
    }
    let mut resp = Response::new(axum::body::Body::from(zip_archive(&files)));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"artifacts.zip\""),
    );
    resp
}

fn delete_job_artifacts(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_job(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    let j = st.collection_mut("jobs").get_mut(&id).expect("job");
    j["artifacts"] = json!([]);
    j["artifact_files"] = json!({});
    no_content()
}

fn job_artifact_file(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let id = match require_job(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let j = get(st, "jobs", &id).cloned().unwrap_or(json!({}));
    let files = artifact_files(&j);
    let path = r.p("artifact_path").trim_start_matches('/').to_owned();
    if path == "tree" {
        // Artifact browser listing (`path`, `recursive`).
        let dir = r.q("path").unwrap_or("").trim_matches('/').to_owned();
        let recursive = r.q("recursive") == Some("true");
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut entries: std::collections::BTreeMap<String, Value> =
            std::collections::BTreeMap::new();
        for (p, c) in &files {
            let Some(rest) = p.strip_prefix(&prefix) else {
                continue;
            };
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() == 1 || recursive {
                if recursive {
                    let mut acc = prefix.clone();
                    for part in &parts[..parts.len() - 1] {
                        acc.push_str(part);
                        acc.push('/');
                        entries.entry(acc.clone()).or_insert_with(|| {
                            json!({"name": part, "path": acc.trim_end_matches('/'), "type": "directory", "size": null, "mode": "040755"})
                        });
                    }
                }
                entries.insert(p.clone(), json!({"name": parts[parts.len() - 1], "path": p, "type": "file", "size": c.len(), "mode": "100644"}));
            } else {
                let d = format!("{prefix}{}", parts[0]);
                entries.entry(format!("{d}/")).or_insert_with(|| json!({"name": parts[0], "path": d, "type": "directory", "size": null, "mode": "040755"}));
            }
        }
        if files.is_empty() {
            return error(axum::http::StatusCode::NOT_FOUND, "404 Not found");
        }
        return paged(r, entries.into_values().collect());
    }
    match files.iter().find(|(p, _)| *p == path) {
        Some((_, c)) => text(StatusCode::OK, "text/plain; charset=utf-8", c.clone()),
        None => error(axum::http::StatusCode::NOT_FOUND, "404 Not found"),
    }
}

// ---------------------------------------------------------------------------
// Triggers

fn render_trigger(st: &Store, base: &str, t: &Value) -> Value {
    let owner = str_of(t, "owner");
    let mine = owner == current_username(st);
    let token = str_of(t, "token");
    json!({
        "id": t["id"],
        "token": if mine { token.to_owned() } else { token.chars().take(4).collect::<String>() },
        "description": t.get("description").cloned().unwrap_or(Value::Null),
        "created_at": t["created_at"],
        "updated_at": t.get("updated_at").unwrap_or(&t["created_at"]),
        "last_used": t.get("last_used").cloned().unwrap_or(Value::Null),
        "expires_at": t.get("expires_at").cloned().unwrap_or(Value::Null),
        "owner": user_opt(st, base, &t["owner"]),
    })
}

fn list_triggers(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = all(st, "pipeline_triggers")
        .into_iter()
        .filter(|(_, t)| proj_of(t) == project)
        .map(|(_, t)| render_trigger(st, &r.base, t))
        .collect();
    paged(r, items)
}

fn require_trigger(st: &Store, r: &Req) -> Result<String, Response> {
    let project = require_project(st, r)?;
    let id = r.p("trigger_id").to_owned();
    match get(st, "pipeline_triggers", &id) {
        Some(t) if proj_of(t) == project => Ok(id),
        _ => Err(not_found("Trigger")),
    }
}

fn get_trigger(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_trigger(st, r) {
        Ok(id) => ok(render_trigger(
            st,
            &r.base,
            get(st, "pipeline_triggers", &id).expect("trigger"),
        )),
        Err(e) => e,
    }
}

fn create_trigger(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(description) = r.s("description") else {
        return missing("description");
    };
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "pipeline_triggers", 1);
    let doc = json!({
        "id": id,
        "project": project,
        "description": description,
        "token": format!("glptt-{}", &fake_sha(&format!("trigger:{project}:{id}"))[..20]),
        "owner": current_username(st),
        "created_at": now,
        "updated_at": now,
        "last_used": null,
        "expires_at": r.s("expires_at"),
    });
    st.collection_mut("pipeline_triggers")
        .insert(id.to_string(), doc.clone());
    created(render_trigger(st, &r.base, &doc))
}

fn update_trigger(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_trigger(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let now = now(w);
    let st = svc_mut(w);
    let t = st
        .collection_mut("pipeline_triggers")
        .get_mut(&id)
        .expect("trigger");
    if let Some(d) = r.s("description") {
        t["description"] = json!(d);
    }
    t["updated_at"] = json!(now);
    let t = t.clone();
    ok(render_trigger(st, &r.base, &t))
}

fn delete_trigger(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_trigger(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    remove_doc(svc_mut(w), "pipeline_triggers", &id);
    no_content()
}

// ---------------------------------------------------------------------------
// Schedules

fn next_run(now: &str, cron: &str) -> String {
    let parts: Vec<&str> = cron.split_whitespace().collect();
    let minute = parts
        .first()
        .and_then(|m| m.parse::<u32>().ok())
        .unwrap_or(0);
    let hour = parts.get(1).and_then(|h| h.parse::<u32>().ok());
    let date = &now[..10];
    let cur_h: u32 = now[11..13].parse().unwrap_or(0);
    let cur_m: u32 = now[14..16].parse().unwrap_or(0);
    match hour {
        Some(h) => {
            if (h, minute) > (cur_h, cur_m) {
                format!("{date}T{h:02}:{minute:02}:00.000Z")
            } else {
                let next = mock_core::clock::LogicalClock::new(&format!("{date}T00:00:00Z"), 0)
                    .map(|c| c.now_unix() + 86_400)
                    .unwrap_or(0);
                let day = mock_core::clock::format_unix(next);
                format!("{}T{h:02}:{minute:02}:00.000Z", &day[..10])
            }
        }
        None => {
            let h = if minute > cur_m {
                cur_h
            } else {
                (cur_h + 1) % 24
            };
            format!("{date}T{h:02}:{minute:02}:00.000Z")
        }
    }
}

fn render_schedule(st: &Store, base: &str, s: &Value, with_vars: bool) -> Value {
    let last = as_u64(&s["last_pipeline_id"])
        .and_then(|id| get(st, "pipelines", &id.to_string()).cloned())
        .or_else(|| {
            all(st, "pipelines")
                .into_iter()
                .filter(|(_, p)| p["schedule_id"] == s["id"])
                .map(|(_, p)| p.clone())
                .max_by_key(|p| as_u64(&p["id"]))
        });
    let mut out = json!({
        "id": s["id"],
        "description": str_of(s, "description"),
        "ref": s.get("ref").cloned().unwrap_or(Value::Null),
        "cron": str_of(s, "cron"),
        "cron_timezone": s.get("cron_timezone").cloned().unwrap_or(json!("UTC")),
        "next_run_at": s.get("next_run_at").cloned().unwrap_or(Value::Null),
        "active": s.get("active").cloned().unwrap_or(json!(true)),
        "created_at": s["created_at"],
        "updated_at": s.get("updated_at").unwrap_or(&s["created_at"]),
        "owner": user_opt(st, base, &s["owner"]),
        "last_pipeline": last.map(|p| json!({"id": p["id"], "sha": p["sha"], "ref": p["ref"], "status": p["status"]})).unwrap_or(Value::Null),
        "inputs": s.get("inputs").cloned().unwrap_or(json!([])),
    });
    if with_vars {
        out["variables"] = Value::Array(
            s.get("variables")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(render_schedule_var)
                .collect(),
        );
    }
    out
}

fn render_schedule_var(v: &Value) -> Value {
    json!({
        "key": str_of(v, "key"),
        "variable_type": v.get("variable_type").cloned().unwrap_or(json!("env_var")),
        "value": v.get("value").cloned().unwrap_or(json!("")),
        "raw": v.get("raw").cloned().unwrap_or(json!(false)),
    })
}

fn list_schedules(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = all(st, "pipeline_schedules")
        .into_iter()
        .filter(|(_, s)| proj_of(s) == project)
        .filter(|(_, s)| match r.q("scope") {
            Some("active") => s.get("active").and_then(Value::as_bool).unwrap_or(true),
            Some("inactive") => !s.get("active").and_then(Value::as_bool).unwrap_or(true),
            _ => true,
        })
        .map(|(_, s)| render_schedule(st, &r.base, s, false))
        .collect();
    paged(r, items)
}

fn require_schedule(st: &Store, r: &Req) -> Result<String, Response> {
    let project = require_project(st, r)?;
    let id = r.p("schedule_id").to_owned();
    match get(st, "pipeline_schedules", &id) {
        Some(s) if proj_of(s) == project => Ok(id),
        _ => Err(not_found("Pipeline Schedule")),
    }
}

fn get_schedule(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_schedule(st, r) {
        Ok(id) => ok(render_schedule(
            st,
            &r.base,
            get(st, "pipeline_schedules", &id).expect("schedule"),
            true,
        )),
        Err(e) => e,
    }
}

fn valid_cron(c: &str) -> bool {
    c.split_whitespace().count() == 5
        && c.split_whitespace().all(|f| {
            f.chars()
                .all(|ch| ch.is_ascii_digit() || "*/,-".contains(ch))
        })
}

fn create_schedule(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(description) = r.s("description") else {
        return missing("description");
    };
    let Some(reference) = r.s("ref") else {
        return missing("ref");
    };
    let Some(cron) = r.s("cron") else {
        return missing("cron");
    };
    if !valid_cron(&cron) {
        return invalid("cron", "is invalid syntax");
    }
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "pipeline_schedules", 1);
    let doc = json!({
        "id": id,
        "project": project,
        "description": description,
        "ref": reference,
        "cron": cron,
        "cron_timezone": r.s("cron_timezone").unwrap_or_else(|| "UTC".into()),
        "next_run_at": next_run(&now, &cron),
        "active": r.bool("active").unwrap_or(true),
        "created_at": now,
        "updated_at": now,
        "owner": current_username(st),
        "variables": [],
        "inputs": r.body.get("inputs").cloned().unwrap_or(json!([])),
    });
    st.collection_mut("pipeline_schedules")
        .insert(id.to_string(), doc.clone());
    created(render_schedule(st, &r.base, &doc, true))
}

fn update_schedule(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_schedule(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    if let Some(c) = r.s("cron")
        && !valid_cron(&c)
    {
        return invalid("cron", "is invalid syntax");
    }
    let now = now(w);
    let st = svc_mut(w);
    let s = st
        .collection_mut("pipeline_schedules")
        .get_mut(&id)
        .expect("schedule");
    for f in ["description", "ref", "cron", "cron_timezone"] {
        if let Some(v) = r.s(f) {
            s[f] = json!(v);
        }
    }
    if let Some(a) = r.bool("active") {
        s["active"] = json!(a);
    }
    if let Some(i) = r.body.get("inputs") {
        s["inputs"] = i.clone();
    }
    let cron = str_of(s, "cron").to_owned();
    s["next_run_at"] = json!(next_run(&now, &cron));
    s["updated_at"] = json!(now);
    let s = s.clone();
    ok(render_schedule(st, &r.base, &s, true))
}

fn delete_schedule(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_schedule(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    remove_doc(svc_mut(w), "pipeline_schedules", &id);
    no_content()
}

fn schedule_pipelines(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let id = match require_schedule(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let mut items: Vec<Value> = all(st, "pipelines")
        .into_iter()
        .filter(|(_, p)| id_is(&p["schedule_id"], &id))
        .map(|(_, p)| p.clone())
        .collect();
    items.sort_by(|a, b| as_u64(&a["id"]).cmp(&as_u64(&b["id"])));
    if r.q("sort") == Some("desc") {
        items.reverse();
    }
    let items = items
        .iter()
        .map(|p| render_pipeline_basic(st, &r.base, p))
        .collect();
    paged(r, items)
}

fn play_schedule(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let id = match require_schedule(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let s = get(st, "pipeline_schedules", &id)
        .cloned()
        .unwrap_or(json!({}));
    let project = proj_of(&s).to_owned();
    let reference = str_of(&s, "ref").to_owned();
    let vars = s.get("variables").cloned().unwrap_or(json!([]));
    match start_pipeline(w, &project, &reference, "schedule", vars, None) {
        Ok(p) => {
            let st = svc_mut(w);
            if let Some(d) = st.collection_mut("pipelines").get_mut(&p["id"].to_string()) {
                d["schedule_id"] = s["id"].clone();
            }
            if let Some(d) = st.collection_mut("pipeline_schedules").get_mut(&id) {
                d["last_pipeline_id"] = p["id"].clone();
            }
            created(json!({"message": "201 Created"}))
        }
        Err(e) => e,
    }
}

fn take_ownership(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_schedule(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let now = now(w);
    let st = svc_mut(w);
    let me = current_username(st);
    let s = st
        .collection_mut("pipeline_schedules")
        .get_mut(&id)
        .expect("schedule");
    s["owner"] = json!(me);
    s["updated_at"] = json!(now);
    let s = s.clone();
    created(render_schedule(st, &r.base, &s, true))
}

fn schedule_vars(st: &Store, id: &str) -> Vec<Value> {
    get(st, "pipeline_schedules", id)
        .and_then(|s| s.get("variables")?.as_array().cloned())
        .unwrap_or_default()
}

fn get_schedule_variable(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let id = match require_schedule(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    match schedule_vars(st, &id)
        .iter()
        .find(|v| str_of(v, "key") == r.p("key"))
    {
        Some(v) => ok(render_schedule_var(v)),
        None => not_found("Variable"),
    }
}

fn create_schedule_variable(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_schedule(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Some(key) = r.s("key") else {
        return missing("key");
    };
    let Some(value) = r.s("value") else {
        return missing("value");
    };
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return invalid("key", "can contain only letters, digits and '_'.");
    }
    if schedule_vars(store(w), &id)
        .iter()
        .any(|v| str_of(v, "key") == key)
    {
        return invalid("key", &format!("({key}) has already been taken"));
    }
    let st = svc_mut(w);
    let var = json!({"key": key, "value": value, "variable_type": r.s("variable_type").unwrap_or_else(|| "env_var".into()), "raw": false});
    let s = st
        .collection_mut("pipeline_schedules")
        .get_mut(&id)
        .expect("schedule");
    let mut vars = s
        .get("variables")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    vars.push(var.clone());
    s["variables"] = Value::Array(vars);
    created(render_schedule_var(&var))
}

fn update_schedule_variable(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_schedule(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let key = r.p("key").to_owned();
    let st = svc_mut(w);
    let s = st
        .collection_mut("pipeline_schedules")
        .get_mut(&id)
        .expect("schedule");
    let mut vars = s
        .get("variables")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(v) = vars.iter_mut().find(|v| str_of(v, "key") == key) else {
        return not_found("Variable");
    };
    if let Some(val) = r.s("value") {
        v["value"] = json!(val);
    }
    if let Some(t) = r.s("variable_type") {
        v["variable_type"] = json!(t);
    }
    let out = render_schedule_var(v);
    s["variables"] = Value::Array(vars);
    ok(out)
}

fn delete_schedule_variable(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_schedule(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let key = r.p("key").to_owned();
    let st = svc_mut(w);
    let s = st
        .collection_mut("pipeline_schedules")
        .get_mut(&id)
        .expect("schedule");
    let mut vars = s
        .get("variables")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(pos) = vars.iter().position(|v| str_of(v, "key") == key) else {
        return not_found("Variable");
    };
    let removed = vars.remove(pos);
    s["variables"] = Value::Array(vars);
    accepted(render_schedule_var(&removed))
}

// ---------------------------------------------------------------------------
// CI lint (a small `.gitlab-ci.yml` reader: stages, jobs, stage/script checks)

struct CiJob {
    name: String,
    stage: String,
    script: String,
    manual: bool,
    allow_failure: bool,
}

struct CiConfig {
    stages: Vec<String>,
    jobs: Vec<CiJob>,
    errors: Vec<String>,
    warnings: Vec<String>,
}

const RESERVED: &[&str] = &[
    "stages",
    "variables",
    "default",
    "include",
    "workflow",
    "image",
    "services",
    "before_script",
    "after_script",
    "cache",
    "spec",
];

fn parse_ci(content: &str) -> CiConfig {
    let mut stages: Vec<String> = Vec::new();
    let mut jobs: Vec<CiJob> = Vec::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut current: Option<CiJob> = None;
    let mut has_script = false;
    let mut in_stages = false;
    let mut in_script = false;
    let flush =
        |job: Option<CiJob>, has_script: bool, jobs: &mut Vec<CiJob>, errors: &mut Vec<String>| {
            if let Some(j) = job {
                if !has_script {
                    errors.push(format!(
                        "jobs:{} config should implement the script:, run:, or trigger: keyword",
                        j.name
                    ));
                }
                jobs.push(j);
            }
        };
    if content.contains('\t') {
        errors.push("(<unknown>): found character that cannot start any token while scanning for the next token at line 1 column 1".into());
    }
    for raw in content.lines() {
        let line = raw.split(" #").next().unwrap_or(raw);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if indent == 0 {
            in_stages = false;
            in_script = false;
            let Some((key, rest)) = t.split_once(':') else {
                errors.push(
                    "(<unknown>): could not find expected ':' while scanning a simple key"
                        .to_string(),
                );
                continue;
            };
            let key = key.trim().trim_matches('"').to_owned();
            let rest = rest.trim();
            if key == "stages" {
                if rest.starts_with('[') {
                    stages = rest
                        .trim_matches(|c| c == '[' || c == ']')
                        .split(',')
                        .map(|s| s.trim().trim_matches('"').to_owned())
                        .filter(|s| !s.is_empty())
                        .collect();
                } else {
                    in_stages = true;
                }
                continue;
            }
            flush(current.take(), has_script, &mut jobs, &mut errors);
            has_script = false;
            if RESERVED.contains(&key.as_str()) || key.starts_with('.') {
                continue;
            }
            current = Some(CiJob {
                name: key,
                stage: "test".into(),
                script: String::new(),
                manual: false,
                allow_failure: false,
            });
            continue;
        }
        if in_stages {
            if let Some(s) = t.strip_prefix("- ") {
                stages.push(s.trim().trim_matches('"').to_owned());
            }
            continue;
        }
        let Some(job) = current.as_mut() else {
            continue;
        };
        if in_script && t.starts_with("- ") && indent >= 2 {
            if job.script.is_empty() {
                job.script = t[2..].trim().to_owned();
            }
            continue;
        }
        in_script = false;
        if let Some((k, v)) = t.split_once(':') {
            let v = v.trim().trim_matches('"');
            match k.trim() {
                "stage" => job.stage = v.to_owned(),
                "script" | "run" | "trigger" | "extends" => {
                    has_script = true;
                    if v.is_empty() {
                        in_script = true;
                    } else {
                        job.script = v.trim_matches(|c| c == '[' || c == ']').to_owned();
                    }
                }
                "when" => job.manual = v == "manual",
                "allow_failure" => job.allow_failure = v == "true",
                "only" | "except" => warnings.push(format!(
                    "jobs:{} uses `only`/`except`, which are not being actively developed",
                    job.name
                )),
                _ => {}
            }
        }
    }
    flush(current.take(), has_script, &mut jobs, &mut errors);
    if stages.is_empty() {
        stages = vec!["build".into(), "test".into(), "deploy".into()];
    }
    let mut all_stages = vec![".pre".to_owned()];
    all_stages.extend(stages.iter().cloned());
    all_stages.push(".post".into());
    for j in &jobs {
        if !all_stages.contains(&j.stage) {
            errors.push(format!(
                "{} job: chosen stage {} does not exist; available stages are {}",
                j.name,
                j.stage,
                all_stages.join(", ")
            ));
        }
    }
    if jobs.is_empty() && errors.is_empty() {
        errors.push("jobs config should contain at least one visible job".into());
    }
    CiConfig {
        stages: all_stages,
        jobs,
        errors,
        warnings,
    }
}

fn lint_result(content: &str, include_jobs: bool) -> Value {
    let c = parse_ci(content);
    let valid = c.errors.is_empty();
    let mut out = json!({
        "valid": valid,
        "errors": c.errors,
        "warnings": c.warnings,
        "merged_yaml": if valid { json!(content) } else { Value::Null },
        "includes": if valid { json!([]) } else { Value::Null },
    });
    if include_jobs && valid {
        out["jobs"] = Value::Array(
            c.jobs
                .iter()
                .map(|j| json!({"name": j.name, "stage": j.stage, "before_script": [], "script": [j.script], "after_script": [], "tag_list": [], "environment": null, "when": if j.manual { "manual" } else { "on_success" }, "allow_failure": j.allow_failure, "only": {"refs": ["branches", "tags"]}, "except": null, "needs": null}))
                .collect(),
        );
    }
    out
}

fn lint(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    if r.path.contains_key("id")
        && let Err(e) = require_project(st, r)
    {
        return e;
    }
    let Some(content) = r.s("content") else {
        return missing("content");
    };
    ok(lint_result(
        &content,
        r.bool("include_jobs").unwrap_or(false),
    ))
}

fn project_lint(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let reference = r
        .q("content_ref")
        .or(r.q("sha"))
        .map(str::to_owned)
        .unwrap_or_else(|| crate::repository::default_branch(st, &project));
    let Some(snap) = crate::repository::resolve_ref(st, &project, &reference) else {
        return not_found("Commit");
    };
    let path = get(st, "projects", &project)
        .and_then(|p| p.get("ci_config_path")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| ".gitlab-ci.yml".into());
    match crate::repository::file_at(st, &project, &snap.branch, &path) {
        Some(c) => ok(lint_result(&c, r.q("include_jobs") == Some("true"))),
        None => ok(
            json!({"valid": false, "errors": ["Please provide content of .gitlab-ci.yml"], "warnings": [], "merged_yaml": null, "includes": null}),
        ),
    }
}

// ---------------------------------------------------------------------------
// CI variables (project and group)

fn var_scope(st: &Store, r: &Req) -> Result<String, Response> {
    if r.uri_path.contains("/groups/") {
        require_group(st, r)
    } else {
        require_project(st, r)
    }
}

fn var_key(scope: &str, key: &str, env: &str) -> String {
    if env == "*" {
        format!("{scope}:{key}")
    } else {
        format!("{scope}:{key}:{env}")
    }
}

fn render_var(v: &Value) -> Value {
    let hidden = v.get("hidden").and_then(Value::as_bool).unwrap_or(false);
    json!({
        "variable_type": v.get("variable_type").cloned().unwrap_or(json!("env_var")),
        "key": str_of(v, "key"),
        "value": if hidden { Value::Null } else { v.get("value").cloned().unwrap_or(json!("")) },
        "hidden": hidden,
        "protected": v.get("protected").cloned().unwrap_or(json!(false)),
        "masked": v.get("masked").cloned().unwrap_or(json!(false)),
        "raw": v.get("raw").cloned().unwrap_or(json!(false)),
        "environment_scope": v.get("environment_scope").cloned().unwrap_or(json!("*")),
        "description": v.get("description").cloned().unwrap_or(Value::Null),
    })
}

fn scope_vars(st: &Store, scope: &str) -> Vec<(String, Value)> {
    let prefix = format!("{scope}:");
    all(st, "variables")
        .into_iter()
        .filter(|(k, v)| {
            k.starts_with(&prefix) && k[prefix.len()..].split(':').next() == Some(str_of(v, "key"))
        })
        .map(|(k, v)| (k.to_owned(), v.clone()))
        .collect()
}

fn find_var(st: &Store, scope: &str, key: &str, r: &Req) -> Result<(String, Value), Response> {
    let env = r.q("filter[environment_scope]");
    let matches: Vec<(String, Value)> = scope_vars(st, scope)
        .into_iter()
        .filter(|(_, v)| str_of(v, "key") == key)
        .filter(|(_, v)| {
            env.is_none_or(|e| {
                v.get("environment_scope")
                    .and_then(Value::as_str)
                    .unwrap_or("*")
                    == e
            })
        })
        .collect();
    match matches.len() {
        0 => Err(not_found("Variable")),
        1 => Ok(matches.into_iter().next().expect("one")),
        _ => Err(error(
            StatusCode::CONFLICT,
            "There are multiple variables with provided parameters. Please use 'filter[environment_scope]'",
        )),
    }
}

fn list_variables(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match var_scope(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let env = r.q("filter[environment_scope]");
    let items = scope_vars(st, &scope)
        .iter()
        .filter(|(_, v)| {
            env.is_none_or(|e| {
                v.get("environment_scope")
                    .and_then(Value::as_str)
                    .unwrap_or("*")
                    == e
            })
        })
        .map(|(_, v)| render_var(v))
        .collect();
    paged(r, items)
}

fn get_variable(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match var_scope(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    match find_var(st, &scope, r.p("key"), r) {
        Ok((_, v)) => ok(render_var(&v)),
        Err(e) => e,
    }
}

fn validate_masked(value: &str) -> bool {
    value.len() >= 8 && !value.contains(char::is_whitespace)
}

fn create_variable(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match var_scope(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let Some(key) = r.s("key") else {
        return missing("key");
    };
    let Some(value) = r.s("value") else {
        return missing("value");
    };
    if key.is_empty()
        || key.len() > 255
        || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return invalid("key", "can contain only letters, digits and '_'.");
    }
    let env = r.s("environment_scope").unwrap_or_else(|| "*".into());
    let masked = r.bool("masked").unwrap_or(false) || r.bool("masked_and_hidden").unwrap_or(false);
    if masked && !validate_masked(&value) {
        return invalid("value", "is invalid");
    }
    let k = var_key(&scope, &key, &env);
    if get(st, "variables", &k).is_some() {
        return invalid("key", &format!("({key}) has already been taken"));
    }
    let st = svc_mut(w);
    let doc = json!({
        "key": key,
        "value": value,
        "variable_type": r.s("variable_type").unwrap_or_else(|| "env_var".into()),
        "protected": r.bool("protected").unwrap_or(false),
        "masked": masked,
        "hidden": r.bool("masked_and_hidden").unwrap_or(false),
        "raw": r.bool("raw").unwrap_or(false),
        "environment_scope": env,
        "description": r.s("description"),
    });
    st.collection_mut("variables").insert(k, doc.clone());
    created(render_var(&doc))
}

fn update_variable(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match var_scope(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let key = r.p("key").to_owned();
    let (k, mut doc) = match find_var(st, &scope, &key, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    if let Some(v) = r.s("value") {
        doc["value"] = json!(v);
    }
    for f in ["variable_type", "description"] {
        if let Some(v) = r.s(f) {
            doc[f] = json!(v);
        }
    }
    for f in ["protected", "masked", "raw"] {
        if let Some(b) = r.bool(f) {
            doc[f] = json!(b);
        }
    }
    if doc["masked"].as_bool().unwrap_or(false) && !validate_masked(str_of(&doc, "value")) {
        return invalid("value", "is invalid");
    }
    let st = svc_mut(w);
    let new_env = r.s("environment_scope");
    remove_doc(st, "variables", &k);
    if let Some(env) = new_env {
        doc["environment_scope"] = json!(env);
    }
    let env = doc
        .get("environment_scope")
        .and_then(Value::as_str)
        .unwrap_or("*")
        .to_owned();
    st.collection_mut("variables")
        .insert(var_key(&scope, &key, &env), doc.clone());
    ok(render_var(&doc))
}

fn delete_variable(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match var_scope(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let (k, _) = match find_var(st, &scope, r.p("key"), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    remove_doc(svc_mut(w), "variables", &k);
    no_content()
}

// ---------------------------------------------------------------------------
// Environments

fn env_slug(name: &str) -> String {
    let mut s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    s.truncate(24);
    format!("{}-{}", s.trim_matches('-'), &fake_sha(name)[..6])
}

fn last_deployment(st: &Store, base: &str, env_id: &Value) -> Value {
    all(st, "deployments")
        .into_iter()
        .filter(|(_, d)| env_matches(st, d, env_id))
        .map(|(_, d)| d.clone())
        .max_by_key(|d| as_u64(&d["id"]))
        .map(|d| {
            json!({
                "id": d["id"], "iid": d.get("iid").cloned().unwrap_or(d["id"].clone()), "status": str_of(&d, "status"),
                "ref": str_of(&d, "ref"), "sha": str_of(&d, "sha"), "created_at": d["created_at"],
                "updated_at": d.get("updated_at").unwrap_or(&d["created_at"]),
                "web_url": format!("{base}/{}/-/environments/{}", proj_of(&d), env_id),
                "user": user_opt(st, base, &d["user"]),
            })
        })
        .unwrap_or(Value::Null)
}

fn env_matches(st: &Store, d: &Value, env_id: &Value) -> bool {
    let env_name =
        get(st, "environments", &env_id.to_string()).map(|e| str_of(e, "name").to_owned());
    env_name.as_deref() == Some(str_of(d, "environment"))
        && get(st, "environments", &env_id.to_string()).is_some_and(|e| proj_of(e) == proj_of(d))
}

fn render_env(st: &Store, base: &str, e: &Value) -> Value {
    let mut out = render_env_full(st, base, e);
    if out["environment_type"] == ""
        && let Some(o) = out.as_object_mut()
    {
        o.remove("environment_type");
    }
    out
}

fn render_env_full(st: &Store, base: &str, e: &Value) -> Value {
    let name = str_of(e, "name");
    json!({
        "id": e["id"],
        "name": name,
        "slug": e.get("slug").cloned().unwrap_or_else(|| json!(env_slug(name))),
        "description": e.get("description").cloned().unwrap_or(Value::Null),
        "external_url": e.get("external_url").cloned().unwrap_or(Value::Null),
        "created_at": e["created_at"],
        "updated_at": e.get("updated_at").unwrap_or(&e["created_at"]),
        "tier": e.get("tier").cloned().unwrap_or(json!(if name.starts_with("prod") { "production" } else if name.starts_with("stag") { "staging" } else { "development" })),
        "state": e.get("state").cloned().unwrap_or(json!("available")),
        "environment_type": name.split_once('/').map(|(t, _)| t).unwrap_or(""),
        "auto_stop_at": e.get("auto_stop_at").cloned().unwrap_or(Value::Null),
        "auto_stop_setting": e.get("auto_stop_setting").cloned().unwrap_or(json!("always")),
        "enable_advanced_logs_querying": false,
        "logs_api_path": format!("/{}/-/logs/k8s.json?environment_name={name}", proj_of(e)),
        "web_url": format!("{base}/{}/-/environments/{}", proj_of(e), e["id"]),
        "last_deployment": last_deployment(st, base, &e["id"]),
    })
}

fn list_environments(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let items = all(st, "environments")
        .into_iter()
        .filter(|(_, e)| proj_of(e) == project)
        .filter(|(_, e)| {
            r.q("name").is_none_or(|n| str_of(e, "name") == n)
                && r.q("search").is_none_or(|s| str_of(e, "name").contains(s))
                && r.q("states").is_none_or(|s| {
                    e.get("state")
                        .and_then(Value::as_str)
                        .unwrap_or("available")
                        == s
                })
        })
        .map(|(_, e)| render_env(st, &r.base, e))
        .collect();
    paged(r, items)
}

fn require_env(st: &Store, r: &Req) -> Result<String, Response> {
    let project = require_project(st, r)?;
    let id = r.p("environment_id").to_owned();
    match get(st, "environments", &id) {
        Some(e) if proj_of(e) == project => Ok(id),
        _ => Err(not_found("Environment")),
    }
}

fn get_environment(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_env(st, r) {
        Ok(id) => ok(render_env(
            st,
            &r.base,
            get(st, "environments", &id).expect("env"),
        )),
        Err(e) => e,
    }
}

fn insert_env(st: &mut Store, project: &str, name: &str, now: &str) -> u64 {
    let id = next_id(st, "environments", 1);
    st.collection_mut("environments").insert(
        id.to_string(),
        json!({"id": id, "project": project, "name": name, "state": "available", "external_url": null, "created_at": now, "updated_at": now}),
    );
    id
}

fn create_environment(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(name) = r.s("name") else {
        return missing("name");
    };
    if all(st, "environments")
        .iter()
        .any(|(_, e)| proj_of(e) == project && str_of(e, "name") == name)
    {
        return invalid("name", "has already been taken");
    }
    let now = now(w);
    let st = svc_mut(w);
    let id = insert_env(st, &project, &name, &now);
    let e = st
        .collection_mut("environments")
        .get_mut(&id.to_string())
        .expect("env");
    for f in ["external_url", "tier", "description"] {
        if let Some(v) = r.s(f) {
            e[f] = json!(v);
        }
    }
    let e = e.clone();
    created(render_env(st, &r.base, &e))
}

fn update_environment(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_env(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let now = now(w);
    let st = svc_mut(w);
    let e = st.collection_mut("environments").get_mut(&id).expect("env");
    for f in ["external_url", "tier", "auto_stop_setting", "description"] {
        if r.has(f) {
            e[f] = r.arg(f).unwrap_or(Value::Null);
        }
    }
    e["updated_at"] = json!(now);
    let e = e.clone();
    ok(render_env(st, &r.base, &e))
}

fn delete_environment(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_env(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let st = store(w);
    if get(st, "environments", &id)
        .and_then(|e| e.get("state")?.as_str())
        .unwrap_or("available")
        != "stopped"
    {
        return error(
            StatusCode::FORBIDDEN,
            "403 Forbidden  - Environment must be stopped before deletion",
        );
    }
    remove_doc(svc_mut(w), "environments", &id);
    no_content()
}

fn stop_environment(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_env(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let now = now(w);
    let st = svc_mut(w);
    let e = st.collection_mut("environments").get_mut(&id).expect("env");
    e["state"] = json!("stopped");
    e["updated_at"] = json!(now);
    let e = e.clone();
    ok(render_env(st, &r.base, &e))
}

fn stop_stale(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(before) = r.s("before") else {
        return missing("before");
    };
    let now = now(w);
    let st = svc_mut(w);
    let ids: Vec<String> = all(st, "environments")
        .iter()
        .filter(|(_, e)| {
            proj_of(e) == project
                && e.get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("available")
                    == "available"
                && str_of(e, "updated_at") < before.as_str()
                && !str_of(e, "name").starts_with("prod")
        })
        .map(|(k, _)| (*k).to_owned())
        .collect();
    for id in ids {
        let e = st.collection_mut("environments").get_mut(&id).expect("env");
        e["state"] = json!("stopped");
        e["updated_at"] = json!(now);
    }
    ok(json!({"message": "Successfully requested stop for all stale environments"}))
}

fn delete_review_apps(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let before = r.s("before").unwrap_or_else(|| "9999".into());
    let limit = r.int("limit").unwrap_or(100) as usize;
    let dry_run = r.bool("dry_run").unwrap_or(true);
    let st = store(w);
    let entries: Vec<(String, Value)> = all(st, "environments")
        .into_iter()
        .filter(|(_, e)| {
            proj_of(e) == project
                && str_of(e, "name").starts_with("review/")
                && str_of(e, "updated_at") < before.as_str()
        })
        .take(limit)
        .map(|(k, e)| (k.to_owned(), e.clone()))
        .collect();
    let scheduled: Vec<Value> = entries
        .iter()
        .map(|(_, e)| json!({"id": e["id"], "name": e["name"], "slug": render_env(st, &r.base, e)["slug"], "external_url": e.get("external_url").cloned().unwrap_or(Value::Null)}))
        .collect();
    if !dry_run {
        let st = svc_mut(w);
        for (k, _) in &entries {
            if let Some(e) = st.collection_mut("environments").get_mut(k) {
                e["auto_delete_at"] = json!("2026-04-01T00:00:00.000Z");
            }
        }
    }
    accepted(json!({"scheduled_entries": scheduled, "unprocessable_entries": []}))
}

// ---------------------------------------------------------------------------
// Deployments

fn render_deployment(st: &Store, base: &str, d: &Value) -> Value {
    let project = proj_of(d);
    let env = all(st, "environments")
        .into_iter()
        .find(|(_, e)| proj_of(e) == project && str_of(e, "name") == str_of(d, "environment"))
        .map(|(_, e)| {
            json!({
                "id": e["id"], "name": str_of(e, "name"), "slug": render_env(st, base, e)["slug"],
                "external_url": e.get("external_url").cloned().unwrap_or(Value::Null),
                "created_at": e["created_at"], "updated_at": e.get("updated_at").unwrap_or(&e["created_at"]),
                "state": e.get("state").cloned().unwrap_or(json!("available")),
                "tier": render_env(st, base, e)["tier"],
            })
        })
        .unwrap_or_else(|| json!({"name": str_of(d, "environment")}));
    let deployable = as_u64(&d["job_id"])
        .and_then(|j| get(st, "jobs", &j.to_string()).cloned())
        .map(|j| render_job(st, base, &j))
        .unwrap_or(Value::Null);
    let approvals: Vec<Value> = d
        .get("approvals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|a| json!({"user": user_opt(st, base, &a["user"]), "status": a["status"], "created_at": a["created_at"], "comment": a.get("comment").cloned().unwrap_or(Value::Null)}))
        .collect();
    let required = as_u64(&d["required_approvals"]).unwrap_or(0);
    let approved = approvals
        .iter()
        .filter(|a| a["status"] == "approved")
        .count() as u64;
    let mut out = json!({
        "id": d["id"],
        "iid": d.get("iid").cloned().unwrap_or(d["id"].clone()),
        "ref": str_of(d, "ref"),
        "sha": str_of(d, "sha"),
        "created_at": d["created_at"],
        "updated_at": d.get("updated_at").unwrap_or(&d["created_at"]),
        "finished_at": d.get("finished_at").cloned().unwrap_or(Value::Null),
        "status": str_of(d, "status"),
        "user": user_opt(st, base, &d["user"]),
        "environment": env,
        "deployable": deployable,
        "pending_approval_count": required.saturating_sub(approved),
        "approvals": approvals,
    });
    if out["user"].is_null()
        && let Some(o) = out.as_object_mut()
    {
        o.remove("user");
    }
    out
}

fn list_deployments(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut items: Vec<Value> = all(st, "deployments")
        .into_iter()
        .filter(|(_, d)| proj_of(d) == project)
        .filter(|(_, d)| {
            r.q("environment")
                .is_none_or(|e| str_of(d, "environment") == e)
                && r.q("status").is_none_or(|s| str_of(d, "status") == s)
                && r.q("sha").is_none_or(|s| str_of(d, "sha") == s)
                && r.q("ref").is_none_or(|s| str_of(d, "ref") == s)
                && r.q("updated_after")
                    .is_none_or(|a| str_of(d, "updated_at") >= a)
                && r.q("updated_before")
                    .is_none_or(|b| str_of(d, "updated_at") <= b)
        })
        .map(|(_, d)| d.clone())
        .collect();
    let field = match r.q("order_by") {
        Some("created_at") => "created_at",
        Some("updated_at") => "updated_at",
        Some("finished_at") => "finished_at",
        Some("ref") => "ref",
        _ => "id",
    };
    items.sort_by(|a, b| {
        let ord = if field == "id" {
            as_u64(&a["id"]).cmp(&as_u64(&b["id"]))
        } else {
            str_of(a, field)
                .cmp(str_of(b, field))
                .then(as_u64(&a["id"]).cmp(&as_u64(&b["id"])))
        };
        if r.q("sort") == Some("desc") {
            ord.reverse()
        } else {
            ord
        }
    });
    let items = items
        .iter()
        .map(|d| render_deployment(st, &r.base, d))
        .collect();
    paged(r, items)
}

fn require_deployment(st: &Store, r: &Req) -> Result<String, Response> {
    let project = require_project(st, r)?;
    let id = r.p("deployment_id").to_owned();
    match get(st, "deployments", &id) {
        Some(d) if proj_of(d) == project => Ok(id),
        _ => Err(not_found("Deployment")),
    }
}

fn get_deployment(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_deployment(st, r) {
        Ok(id) => ok(render_deployment(
            st,
            &r.base,
            get(st, "deployments", &id).expect("deployment"),
        )),
        Err(e) => e,
    }
}

const DEPLOY_STATUSES: &[&str] = &[
    "running", "success", "failed", "canceled", "created", "blocked",
];

fn create_deployment(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    for f in ["environment", "sha", "ref", "tag", "status"] {
        if !r.has(f) {
            return missing(f);
        }
    }
    let status = r.s("status").unwrap_or_default();
    if !DEPLOY_STATUSES.contains(&status.as_str()) {
        return bad_request("status does not have a valid value");
    }
    let env = r.s("environment").unwrap_or_default();
    let now = now(w);
    let st = svc_mut(w);
    if !all(st, "environments")
        .iter()
        .any(|(_, e)| proj_of(e) == project && str_of(e, "name") == env)
    {
        insert_env(st, &project, &env, &now);
    }
    let id = next_id(st, "deployments", 1);
    let iid = all(st, "deployments")
        .iter()
        .filter(|(_, d)| proj_of(d) == project)
        .filter_map(|(_, d)| as_u64(&d["iid"]))
        .max()
        .unwrap_or(0)
        + 1;
    let doc = json!({
        "id": id,
        "iid": iid,
        "project": project,
        "environment": env,
        "ref": r.s("ref"),
        "sha": r.s("sha"),
        "tag": r.bool("tag").unwrap_or(false),
        "status": status,
        "user": current_username(st),
        "created_at": now,
        "updated_at": now,
        "finished_at": if ["success", "failed", "canceled"].contains(&status.as_str()) { json!(now) } else { Value::Null },
    });
    st.collection_mut("deployments")
        .insert(id.to_string(), doc.clone());
    created(render_deployment(st, &r.base, &doc))
}

fn update_deployment(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_deployment(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Some(status) = r.s("status") else {
        return missing("status");
    };
    if !DEPLOY_STATUSES.contains(&status.as_str()) {
        return bad_request("status does not have a valid value");
    }
    let now = now(w);
    let st = svc_mut(w);
    let d = st
        .collection_mut("deployments")
        .get_mut(&id)
        .expect("deployment");
    d["status"] = json!(status);
    d["updated_at"] = json!(now);
    if ["success", "failed", "canceled"].contains(&status.as_str()) {
        d["finished_at"] = json!(now);
    }
    let d = d.clone();
    ok(render_deployment(st, &r.base, &d))
}

fn delete_deployment(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_deployment(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    if str_of(
        get(store(w), "deployments", &id).expect("deployment"),
        "status",
    ) == "running"
    {
        return bad_request("Cannot destroy running deployment");
    }
    remove_doc(svc_mut(w), "deployments", &id);
    no_content()
}

fn deployment_mrs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let id = match require_deployment(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let d = get(st, "deployments", &id).cloned().unwrap_or(json!({}));
    let items = value_list(&d["merge_requests"])
        .iter()
        .filter(|k| get(st, "merge_requests", k).is_some())
        .map(|k| crate::merge_requests::render(st, &r.base, k))
        .collect();
    paged(r, items)
}

fn approve_deployment(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_deployment(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Some(status) = r.s("status") else {
        return missing("status");
    };
    if !["approved", "rejected"].contains(&status.as_str()) {
        return bad_request("status does not have a valid value");
    }
    let d = get(store(w), "deployments", &id)
        .cloned()
        .unwrap_or(json!({}));
    if str_of(&d, "status") != "blocked" {
        return bad_request("Deployment is not blocked");
    }
    let now = now(w);
    let st = svc_mut(w);
    let me = current_username(st);
    let approval =
        json!({"user": me, "status": status, "created_at": now, "comment": r.s("comment")});
    let doc = st
        .collection_mut("deployments")
        .get_mut(&id)
        .expect("deployment");
    let mut approvals = doc
        .get("approvals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    approvals.retain(|a| str_of(a, "user") != me);
    approvals.push(approval.clone());
    let required = as_u64(&doc["required_approvals"]).unwrap_or(1);
    let approved = approvals
        .iter()
        .filter(|a| a["status"] == "approved")
        .count() as u64;
    doc["approvals"] = Value::Array(approvals);
    if status == "rejected" {
        doc["status"] = json!("failed");
    } else if approved >= required {
        doc["status"] = json!("running");
    }
    doc["updated_at"] = json!(now);
    created(
        json!({"user": user_basic(st, &r.base, &me), "status": status, "created_at": now, "comment": approval["comment"]}),
    )
}

// ---------------------------------------------------------------------------
// Runners

fn runner_basic(r: &Value) -> Value {
    json!({
        "id": r["id"],
        "description": str_of(r, "description"),
        "ip_address": null,
        "active": !r.get("paused").and_then(Value::as_bool).unwrap_or(false),
        "paused": r.get("paused").cloned().unwrap_or(json!(false)),
        "is_shared": r.get("is_shared").cloned().unwrap_or(json!(false)),
        "runner_type": r.get("runner_type").cloned().unwrap_or(json!("project_type")),
        "name": r.get("name").cloned().unwrap_or(json!("gitlab-runner")),
        "online": r.get("status").and_then(Value::as_str).unwrap_or("online") == "online",
        "status": r.get("status").cloned().unwrap_or(json!("online")),
    })
}

fn runner_available(st: &Store, r: &Value, project: &str) -> bool {
    if r.get("is_shared").and_then(Value::as_bool).unwrap_or(false)
        || str_of(r, "runner_type") == "instance_type"
    {
        return true;
    }
    if value_list(&r["projects"]).iter().any(|p| p == project) {
        return true;
    }
    let _ = st;
    value_list(&r["groups"])
        .iter()
        .any(|g| project.starts_with(&format!("{g}/")))
}

fn project_runners(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let tags = r.list("tag_list").unwrap_or_default();
    let items =
        all(st, "runners")
            .into_iter()
            .filter(|(_, x)| runner_available(st, x, &project))
            .filter(|(_, x)| {
                r.q("type").is_none_or(|t| {
                    x.get("runner_type")
                        .and_then(Value::as_str)
                        .unwrap_or("project_type")
                        == t
                }) && r.q("status").is_none_or(|s| {
                    x.get("status").and_then(Value::as_str).unwrap_or("online") == s
                }) && r.q("paused").is_none_or(|p| {
                    (p == "true") == x.get("paused").and_then(Value::as_bool).unwrap_or(false)
                }) && tags.iter().all(|t| value_list(&x["tag_list"]).contains(t))
            })
            .map(|(_, x)| runner_basic(x))
            .collect();
    paged(r, items)
}

fn owned_runners(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let items = all(st, "runners")
        .into_iter()
        .filter(|(_, x)| !x.get("is_shared").and_then(Value::as_bool).unwrap_or(false))
        .map(|(_, x)| runner_basic(x))
        .collect();
    paged(r, items)
}

fn get_runner(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let Some(x) = get(st, "runners", r.p("runner_id")) else {
        return not_found("Runner");
    };
    let mut out = runner_basic(x);
    let projects: Vec<Value> = value_list(&x["projects"])
        .iter()
        .filter(|p| get(st, "projects", p).is_some())
        .map(|p| {
            let full = render_project(st, &r.base, p);
            json!({"id": full["id"], "name": full["name"], "name_with_namespace": full["name_with_namespace"], "path": full["path"], "path_with_namespace": full["path_with_namespace"]})
        })
        .collect();
    let groups: Vec<Value> = value_list(&x["groups"])
        .iter()
        .filter(|g| get(st, "groups", g).is_some())
        .map(|g| json!({"id": group_id(st, g), "name": get(st, "groups", g).map(|d| str_of(d, "name").to_owned()), "web_url": format!("{}/groups/{g}", r.base)}))
        .collect();
    merge_into(
        &mut out,
        json!({
            "architecture": x.get("architecture").cloned().unwrap_or(json!("amd64")),
            "platform": x.get("platform").cloned().unwrap_or(json!("linux")),
            "version": x.get("version").cloned().unwrap_or(json!("17.11.0")),
            "revision": x.get("revision").cloned().unwrap_or(json!("8ec04662")),
            "contacted_at": x.get("contacted_at").cloned().unwrap_or(Value::Null),
            "maintenance_note": x.get("maintenance_note").cloned().unwrap_or(Value::Null),
            "tag_list": x.get("tag_list").cloned().unwrap_or(json!([])),
            "run_untagged": x.get("run_untagged").cloned().unwrap_or(json!(true)),
            "locked": x.get("locked").cloned().unwrap_or(json!(false)),
            "maximum_timeout": x.get("maximum_timeout").cloned().unwrap_or(json!(3600)),
            "access_level": x.get("access_level").cloned().unwrap_or(json!("not_protected")),
            "projects": projects,
            "groups": groups,
        }),
    );
    ok(out)
}

// ---------------------------------------------------------------------------
// Commit statuses

fn render_status(st: &Store, base: &str, s: &Value) -> Value {
    json!({
        "id": s["id"],
        "sha": str_of(s, "sha"),
        "ref": s.get("ref").cloned().unwrap_or(Value::Null),
        "status": str_of(s, "status"),
        "name": s.get("name").cloned().unwrap_or(json!("default")),
        "target_url": s.get("target_url").cloned().unwrap_or(Value::Null),
        "description": s.get("description").cloned().unwrap_or(Value::Null),
        "created_at": s["created_at"],
        "started_at": s.get("started_at").cloned().unwrap_or(Value::Null),
        "finished_at": s.get("finished_at").cloned().unwrap_or(Value::Null),
        "allow_failure": s.get("allow_failure").cloned().unwrap_or(json!(false)),
        "coverage": s.get("coverage").cloned().unwrap_or(Value::Null),
        "pipeline_id": s.get("pipeline_id").cloned().unwrap_or(Value::Null),
        "author": user_opt(st, base, &s["author"]),
    })
}

fn list_statuses(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(snap) = crate::repository::resolve_ref(st, &project, r.p("sha")) else {
        return not_found("Commit");
    };
    let mut items: Vec<Value> = all(st, "commit_statuses")
        .into_iter()
        .filter(|(_, s)| proj_of(s) == project && str_of(s, "sha") == snap.sha)
        .map(|(_, s)| s.clone())
        .collect();
    // Jobs of pipelines on this sha are commit statuses too.
    for (_, j) in all(st, "jobs") {
        let pid = j["pipeline_id"].to_string();
        if let Some(p) = get(st, "pipelines", &pid)
            && proj_of(p) == project
            && str_of(p, "sha") == snap.sha
            && (r.q("all") == Some("true") || !j["retried"].as_bool().unwrap_or(false))
        {
            items.push(json!({
                    "id": j["id"], "sha": snap.sha, "ref": p["ref"], "status": j["status"], "name": j["name"],
                    "target_url": format!("{}/{project}/-/jobs/{}", r.base, j["id"]), "description": null,
                    "created_at": j["created_at"], "started_at": j.get("started_at").cloned().unwrap_or(Value::Null),
                    "finished_at": j.get("finished_at").cloned().unwrap_or(Value::Null),
                    "allow_failure": j.get("allow_failure").cloned().unwrap_or(json!(false)), "coverage": null,
                    "pipeline_id": p["id"], "author": j.get("user").cloned().unwrap_or(Value::Null), "stage": j["stage"],
                }));
        }
    }
    let items: Vec<Value> = items
        .into_iter()
        .filter(|s| {
            r.q("ref").is_none_or(|x| str_of(s, "ref") == x)
                && r.q("name").is_none_or(|x| str_of(s, "name") == x)
                && r.q("stage")
                    .is_none_or(|x| s.get("stage").and_then(Value::as_str).unwrap_or("test") == x)
                && r.q("pipeline_id")
                    .is_none_or(|x| id_is(&s["pipeline_id"], x))
        })
        .map(|s| render_status(st, &r.base, &s))
        .collect();
    paged(r, items)
}

fn create_status(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(state) = r.s("state") else {
        return missing("state");
    };
    if ![
        "pending", "running", "success", "failed", "canceled", "skipped",
    ]
    .contains(&state.as_str())
    {
        return bad_request("state does not have a valid value");
    }
    let Some(snap) = crate::repository::resolve_ref(st, &project, r.p("sha")) else {
        return not_found("Commit");
    };
    let name = r
        .s("name")
        .or_else(|| r.s("context"))
        .unwrap_or_else(|| "default".into());
    let now = now(w);
    let st = svc_mut(w);
    let existing = all(st, "commit_statuses")
        .iter()
        .find(|(_, s)| {
            proj_of(s) == project && str_of(s, "sha") == snap.sha && str_of(s, "name") == name
        })
        .map(|(k, _)| (*k).to_owned());
    let id = existing
        .clone()
        .unwrap_or_else(|| next_id(st, "commit_statuses", 1).to_string());
    let doc = json!({
        "id": id.parse::<u64>().unwrap_or(0),
        "project": project,
        "sha": snap.sha,
        "ref": r.s("ref").unwrap_or_else(|| snap.branch.clone()),
        "status": state,
        "name": name,
        "target_url": r.s("target_url"),
        "description": r.s("description"),
        "coverage": r.arg("coverage").and_then(|c| c.as_str().and_then(|s| s.parse::<f64>().ok()).or(c.as_f64())),
        "pipeline_id": r.int("pipeline_id"),
        "created_at": now,
        "started_at": if state == "running" { json!(now) } else { Value::Null },
        "finished_at": if ["success", "failed", "canceled", "skipped"].contains(&state.as_str()) { json!(now) } else { Value::Null },
        "author": current_username(st),
    });
    st.collection_mut("commit_statuses").insert(id, doc.clone());
    created(render_status(st, &r.base, &doc))
}
