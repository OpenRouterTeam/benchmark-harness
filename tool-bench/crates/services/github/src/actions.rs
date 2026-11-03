//! GitHub Actions (workflows, runs, jobs, logs, artifacts, usage, dispatch, rerun,
//! cancel), commit statuses, and check runs.
//!
//! Log and artifact downloads redirect (302) to `/_downloads/...` on the same host,
//! which the server then fetches with a plain HTTP client.

use axum::{
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use mock_core::world::ServiceState;
use serde_json::{Value, json};

use crate::{
    Ctx, git,
    render::{self, all, get, pr_head_sha, repo_key, user_ref, with_prefix},
    util::{
        add_secs, arr, created, error, login_of, next_id, no_content, node_id, not_found, ok,
        opt_s, paged_wrapped, s, status_json, u, unix, unprocessable, validation, with_header,
    },
};

pub fn route(ctx: &mut Ctx, segs: &[&str]) -> Option<Response> {
    let m = ctx.req.method.clone();
    let get_ = m == Method::GET;
    let ["repos", o, r, rest @ ..] = segs else {
        return None;
    };
    let first = *rest.first()?;
    let commit_sub = rest.len() >= 3
        && first == "commits"
        && matches!(
            rest[rest.len() - 1],
            "status" | "statuses" | "check-runs" | "check-suites" | "pulls"
        );
    if !matches!(
        first,
        "actions" | "statuses" | "check-runs" | "check-suites"
    ) && !commit_sub
    {
        return None;
    }
    let Some(repo) = repo_key(ctx.svc, o, r) else {
        return Some(not_found());
    };
    Some(match rest {
        ["actions", "workflows"] if get_ => {
            let wfs: Vec<Value> = workflows(ctx.svc, &repo)
                .iter()
                .map(|w| workflow_json(ctx, w))
                .collect();
            paged_wrapped(ctx.req, "workflows", wfs)
        }
        ["actions", "workflows", id] if get_ => match find_workflow(ctx.svc, &repo, id) {
            Some(w) => ok(workflow_json(ctx, &w)),
            None => not_found(),
        },
        ["actions", "workflows", id, "runs"] if get_ => match find_workflow(ctx.svc, &repo, id) {
            Some(w) => list_runs(ctx, &repo, Some(u(&w, "id"))),
            None => not_found(),
        },
        ["actions", "workflows", id, "dispatches"] if m == Method::POST => dispatch(ctx, &repo, id),
        ["actions", "workflows", id, "timing"] if get_ => match find_workflow(ctx.svc, &repo, id) {
            Some(_) => ok(json!({"billable": {}})),
            None => not_found(),
        },
        ["actions", "workflows", id, op @ ("enable" | "disable")] if m == Method::PUT => {
            let Some(w) = find_workflow(ctx.svc, &repo, id) else {
                return Some(not_found());
            };
            let key = u(&w, "id").to_string();
            let state = if *op == "enable" {
                "active"
            } else {
                "disabled_manually"
            };
            if let Some(d) = ctx.svc.store.collection_mut("workflows").get_mut(&key) {
                d["state"] = json!(state);
            }
            no_content()
        }
        ["actions", "runs"] if get_ => list_runs(ctx, &repo, None),
        ["actions", "runs", id] if get_ => match run_doc(ctx.svc, &repo, id) {
            Some(run) => ok(run_json(ctx, &run)),
            None => not_found(),
        },
        ["actions", "runs", id] if m == Method::DELETE => match run_doc(ctx.svc, &repo, id) {
            Some(_) => {
                ctx.svc.store.collection_mut("workflow_runs").remove(id);
                no_content()
            }
            None => not_found(),
        },
        ["actions", "runs", id, "jobs"] if get_ => list_jobs(ctx, &repo, id),
        ["actions", "runs", id, "attempts", attempt, "jobs"] if get_ => {
            list_jobs_attempt(ctx, &repo, id, attempt)
        }
        ["actions", "runs", id, "artifacts"] if get_ => match run_doc(ctx.svc, &repo, id) {
            Some(_) => {
                let name = ctx.req.q("name");
                let arts: Vec<Value> = all(ctx.svc, "artifacts")
                    .iter()
                    .filter(|(_, a)| s(a, "repo") == repo && u(a, "run_id").to_string() == *id)
                    .filter(|(_, a)| name.is_none_or(|n| s(a, "name") == n))
                    .map(|(_, a)| artifact_json(ctx, a))
                    .collect();
                paged_wrapped(ctx.req, "artifacts", arts)
            }
            None => not_found(),
        },
        ["actions", "runs", id, "logs"] if get_ => match run_doc(ctx.svc, &repo, id) {
            Some(run)
                if !run
                    .get("logs_deleted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false) =>
            {
                redirect(ctx, &format!("runs/{repo}/{id}"))
            }
            Some(_) => error(StatusCode::GONE, "Logs have been deleted"),
            None => not_found(),
        },
        ["actions", "runs", id, "logs"] if m == Method::DELETE => match run_doc(ctx.svc, &repo, id)
        {
            Some(_) => {
                if let Some(d) = ctx.svc.store.collection_mut("workflow_runs").get_mut(id) {
                    d["logs_deleted"] = json!(true);
                }
                no_content()
            }
            None => not_found(),
        },
        ["actions", "runs", id, "timing"] if get_ => match run_doc(ctx.svc, &repo, id) {
            Some(run) => ok(run_timing(ctx, &run)),
            None => not_found(),
        },
        ["actions", "runs", id, "rerun"] if m == Method::POST => rerun(ctx, &repo, id, false),
        ["actions", "runs", id, "rerun-failed-jobs"] if m == Method::POST => {
            rerun(ctx, &repo, id, true)
        }
        ["actions", "runs", id, "cancel"] if m == Method::POST => cancel(ctx, &repo, id),
        ["actions", "jobs", id] if get_ => match job_doc(ctx.svc, &repo, id) {
            Some(j) => ok(job_json(ctx, &j)),
            None => not_found(),
        },
        ["actions", "jobs", id, "logs"] if get_ => match job_doc(ctx.svc, &repo, id) {
            Some(_) => redirect(ctx, &format!("jobs/{repo}/{id}")),
            None => not_found(),
        },
        ["actions", "jobs", id, "rerun"] if m == Method::POST => {
            let Some(job) = job_doc(ctx.svc, &repo, id) else {
                return Some(not_found());
            };
            let run_id = u(&job, "run_id").to_string();
            rerun(ctx, &repo, &run_id, true)
        }
        ["actions", "artifacts"] if get_ => {
            let name = ctx.req.q("name");
            let arts: Vec<Value> = all(ctx.svc, "artifacts")
                .iter()
                .filter(|(_, a)| s(a, "repo") == repo)
                .filter(|(_, a)| name.is_none_or(|n| s(a, "name") == n))
                .map(|(_, a)| artifact_json(ctx, a))
                .collect();
            paged_wrapped(ctx.req, "artifacts", arts)
        }
        ["actions", "artifacts", id] if get_ => match artifact_doc(ctx.svc, &repo, id) {
            Some(a) => ok(artifact_json(ctx, &a)),
            None => not_found(),
        },
        ["actions", "artifacts", id, "zip"] if get_ => match artifact_doc(ctx.svc, &repo, id) {
            Some(a) if !a.get("expired").and_then(Value::as_bool).unwrap_or(false) => {
                redirect(ctx, &format!("artifacts/{repo}/{id}"))
            }
            Some(_) => error(StatusCode::GONE, "Artifact has expired"),
            None => not_found(),
        },
        ["statuses", sha] if m == Method::POST => create_status(ctx, &repo, sha),
        ["statuses", sha] if get_ => list_statuses(ctx, &repo, sha),
        ["commits", reference @ .., "status"] if get_ => {
            combined_status(ctx, &repo, &reference.join("/"))
        }
        ["commits", reference @ .., "statuses"] if get_ => {
            list_statuses(ctx, &repo, &reference.join("/"))
        }
        ["commits", reference @ .., "check-runs"] if get_ => {
            check_runs_for_ref(ctx, &repo, &reference.join("/"))
        }
        ["commits", reference @ .., "check-suites"] if get_ => {
            if git::resolve_ref(ctx.svc, &repo, &reference.join("/")).is_some() {
                ok(json!({"total_count": 0, "check_suites": []}))
            } else {
                not_found()
            }
        }
        ["commits", reference @ .., "pulls"] if get_ => {
            commit_pulls(ctx, &repo, &reference.join("/"))
        }
        ["check-runs", id] if get_ => match all_check_runs(ctx.svc, &repo)
            .into_iter()
            .find(|c| u(c, "id").to_string() == *id)
        {
            Some(c) => ok(check_run_json(ctx, &repo, &c)),
            None => not_found(),
        },
        _ => not_found(),
    })
}

fn redirect(ctx: &Ctx, path: &str) -> Response {
    let token = crate::sha1::sha1_hex(format!("{path}{}", ctx.now).as_bytes());
    let url = format!("{}/_downloads/{path}?token={}", ctx.base(), &token[..20]);
    with_header(StatusCode::FOUND.into_response(), header::LOCATION, &url)
}

/// `GET /_downloads/{jobs|runs|artifacts}/{owner}/{repo}/{id}`.
pub fn download(ctx: &mut Ctx, segs: &[&str]) -> Response {
    let [kind, o, r, id] = segs else {
        return not_found();
    };
    let Some(repo) = repo_key(ctx.svc, o, r) else {
        return not_found();
    };
    let text = match *kind {
        "jobs" => match job_doc(ctx.svc, &repo, id) {
            Some(j) => job_log(ctx.svc, &j),
            None => return not_found(),
        },
        "runs" => {
            let Some(run) = run_doc(ctx.svc, &repo, id) else {
                return not_found();
            };
            let mut out = String::new();
            for j in jobs_of_run(ctx.svc, &repo, u(&run, "id"), true) {
                out.push_str(&format!("==> {}.txt <==\n", s(&j, "name")));
                out.push_str(&job_log(ctx.svc, &j));
                out.push('\n');
            }
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/zip")],
                out,
            )
                .into_response();
        }
        "artifacts" => {
            let Some(a) = artifact_doc(ctx.svc, &repo, id) else {
                return not_found();
            };
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/zip")],
                s(&a, "content").to_owned(),
            )
                .into_response();
        }
        _ => return not_found(),
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        text,
    )
        .into_response()
}

// ---------------------------------------------------------------- workflows

fn workflows(svc: &ServiceState, repo: &str) -> Vec<Value> {
    let mut v: Vec<Value> = all(svc, "workflows")
        .into_iter()
        .filter(|(_, w)| s(w, "repo") == repo)
        .map(|(_, w)| w.clone())
        .collect();
    v.sort_by_key(|w| u(w, "id"));
    v
}

fn find_workflow(svc: &ServiceState, repo: &str, id: &str) -> Option<Value> {
    workflows(svc, repo).into_iter().find(|w| {
        u(w, "id").to_string() == id
            || s(w, "path") == id
            || s(w, "path").rsplit('/').next() == Some(id)
    })
}

fn workflow_json(ctx: &Ctx, w: &Value) -> Value {
    let repo = s(w, "repo");
    let id = u(w, "id");
    let path = s(w, "path");
    let file = path.rsplit('/').next().unwrap_or(path);
    json!({
        "id": id,
        "node_id": opt_s(w, "node_id").unwrap_or_else(|| node_id("W", &id.to_string())),
        "name": s(w, "name"),
        "path": path,
        "state": opt_s(w, "state").unwrap_or_else(|| "active".into()),
        "created_at": w.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": w.get("updated_at").cloned().unwrap_or(Value::Null),
        "url": format!("{}/repos/{repo}/actions/workflows/{id}", ctx.req.api()),
        "html_url": format!("{}/{repo}/blob/main/{path}", ctx.base()),
        "badge_url": format!("{}/{repo}/workflows/{}/badge.svg", ctx.base(), file.trim_end_matches(".yml").trim_end_matches(".yaml")),
    })
}

// ---------------------------------------------------------------- runs

fn run_doc(svc: &ServiceState, repo: &str, id: &str) -> Option<Value> {
    get(svc, "workflow_runs", id)
        .filter(|r| s(r, "repo") == repo)
        .cloned()
        .or_else(|| {
            all(svc, "workflow_runs")
                .into_iter()
                .find(|(_, r)| s(r, "repo") == repo && u(r, "id").to_string() == id)
                .map(|(_, r)| r.clone())
        })
}

fn run_key(svc: &ServiceState, repo: &str, id: &str) -> Option<String> {
    all(svc, "workflow_runs")
        .into_iter()
        .find(|(k, r)| s(r, "repo") == repo && (*k == id || u(r, "id").to_string() == id))
        .map(|(k, _)| k.to_owned())
}

fn run_json(ctx: &Ctx, run: &Value) -> Value {
    let repo = s(run, "repo");
    let id = u(run, "id");
    let api = format!("{}/repos/{repo}", ctx.req.api());
    let wf = all(ctx.svc, "workflows")
        .into_iter()
        .find(|(_, w)| u(w, "id") == u(run, "workflow_id"))
        .map(|(_, w)| w.clone())
        .unwrap_or_default();
    let head_sha = s(run, "head_sha");
    let head_commit = git::commit(ctx.svc, repo, head_sha)
        .map(|c| {
            json!({
                "id": head_sha,
                "tree_id": git::file_map(ctx.svc, repo, head_sha).map(|m| git::tree_sha(&m, "")).unwrap_or_default(),
                "message": s(c, "message"),
                "timestamp": git::commit_date(c),
                "author": {"name": c.get("author").and_then(|a| a.get("name")).cloned().unwrap_or(Value::Null), "email": c.get("author").and_then(|a| a.get("email")).cloned().unwrap_or(Value::Null)},
                "committer": {"name": c.get("committer").and_then(|a| a.get("name")).cloned().unwrap_or(Value::Null), "email": c.get("committer").and_then(|a| a.get("email")).cloned().unwrap_or(Value::Null)},
            })
        })
        .unwrap_or(Value::Null);
    let prs: Vec<Value> = arr(run, "pull_requests")
        .iter()
        .filter_map(Value::as_u64)
        .filter_map(|n| {
            let key = format!("{repo}#{n}");
            let p = get(ctx.svc, "pulls", &key)?;
            let head = pr_head_sha(ctx.svc, repo, p);
            Some(json!({
                "url": format!("{api}/pulls/{n}"),
                "id": u(p, "id"),
                "number": n,
                "head": {"ref": render::pr_head_ref(p), "sha": head, "repo": {"id": get(ctx.svc, "repos", repo).map(|r| u(r, "id")).unwrap_or(0), "url": api, "name": repo.split('/').nth(1).unwrap_or("")}},
                "base": {"ref": render::pr_base_ref(p), "sha": render::pr_base_sha(ctx.svc, repo, p), "repo": {"id": get(ctx.svc, "repos", repo).map(|r| u(r, "id")).unwrap_or(0), "url": api, "name": repo.split('/').nth(1).unwrap_or("")}},
            }))
        })
        .collect();
    let actor = run
        .get("actor")
        .map(|a| user_ref(ctx.svc, ctx.base(), a))
        .unwrap_or(Value::Null);
    json!({
        "id": id,
        "name": opt_s(run, "name").unwrap_or_else(|| s(&wf, "name").to_owned()),
        "node_id": opt_s(run, "node_id").unwrap_or_else(|| node_id("WFR", &id.to_string())),
        "head_branch": run.get("head_branch").cloned().unwrap_or(Value::Null),
        "head_sha": head_sha,
        "path": opt_s(run, "path").unwrap_or_else(|| s(&wf, "path").to_owned()),
        "display_title": opt_s(run, "display_title").unwrap_or_else(|| git::commit(ctx.svc, repo, head_sha).map(|c| s(c, "message").lines().next().unwrap_or("").to_owned()).unwrap_or_default()),
        "run_number": u(run, "run_number"),
        "event": opt_s(run, "event").unwrap_or_else(|| "push".into()),
        "status": opt_s(run, "status").unwrap_or_else(|| "completed".into()),
        "conclusion": run.get("conclusion").cloned().unwrap_or(Value::Null),
        "workflow_id": u(run, "workflow_id"),
        "check_suite_id": id * 10,
        "check_suite_node_id": node_id("CS", &id.to_string()),
        "url": format!("{api}/actions/runs/{id}"),
        "html_url": format!("{}/{repo}/actions/runs/{id}", ctx.base()),
        "pull_requests": prs,
        "created_at": run.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": run.get("updated_at").cloned().unwrap_or(Value::Null),
        "actor": actor,
        "run_attempt": u(run, "run_attempt").max(1),
        "referenced_workflows": [],
        "run_started_at": run.get("run_started_at").cloned().or_else(|| run.get("created_at").cloned()).unwrap_or(Value::Null),
        "triggering_actor": run.get("triggering_actor").map(|a| user_ref(ctx.svc, ctx.base(), a)).unwrap_or_else(|| actor.clone()),
        "jobs_url": format!("{api}/actions/runs/{id}/jobs"),
        "logs_url": format!("{api}/actions/runs/{id}/logs"),
        "check_suite_url": format!("{api}/check-suites/{}", id * 10),
        "artifacts_url": format!("{api}/actions/runs/{id}/artifacts"),
        "cancel_url": format!("{api}/actions/runs/{id}/cancel"),
        "rerun_url": format!("{api}/actions/runs/{id}/rerun"),
        "previous_attempt_url": if u(run, "run_attempt") > 1 { json!(format!("{api}/actions/runs/{id}/attempts/{}", u(run, "run_attempt") - 1)) } else { Value::Null },
        "workflow_url": format!("{api}/actions/workflows/{}", u(run, "workflow_id")),
        "head_commit": head_commit,
        "repository": render::repo_min(ctx.svc, ctx.base(), repo),
        "head_repository": render::repo_min(ctx.svc, ctx.base(), repo),
    })
}

fn list_runs(ctx: &Ctx, repo: &str, workflow: Option<u64>) -> Response {
    let actor = ctx.req.q("actor");
    let branch = ctx.req.q("branch");
    let event = ctx.req.q("event");
    let status = ctx.req.q("status");
    let head_sha = ctx.req.q("head_sha");
    let mut runs: Vec<Value> = all(ctx.svc, "workflow_runs")
        .into_iter()
        .map(|(_, r)| r.clone())
        .filter(|r| s(r, "repo") == repo)
        .filter(|r| workflow.is_none_or(|w| u(r, "workflow_id") == w))
        .filter(|r| {
            actor.is_none_or(|a| {
                login_of(r.get("actor").unwrap_or(&Value::Null)).eq_ignore_ascii_case(a)
            })
        })
        .filter(|r| branch.is_none_or(|b| s(r, "head_branch") == b))
        .filter(|r| event.is_none_or(|e| s(r, "event") == e))
        .filter(|r| {
            status.is_none_or(|st| {
                s(r, "status") == st || r.get("conclusion").and_then(Value::as_str) == Some(st)
            })
        })
        .filter(|r| head_sha.is_none_or(|h| s(r, "head_sha") == h))
        .collect();
    runs.sort_by_key(|r| std::cmp::Reverse((unix(s(r, "created_at")).unwrap_or(0), u(r, "id"))));
    let out: Vec<Value> = runs.iter().map(|r| run_json(ctx, r)).collect();
    paged_wrapped(ctx.req, "workflow_runs", out)
}

fn run_timing(ctx: &Ctx, run: &Value) -> Value {
    let jobs = jobs_of_run(ctx.svc, s(run, "repo"), u(run, "id"), true);
    let mut total = 0i64;
    let job_runs: Vec<Value> = jobs
        .iter()
        .map(|j| {
            let d = (unix(s(j, "completed_at")).unwrap_or(0)
                - unix(s(j, "started_at")).unwrap_or(0))
            .max(0)
                * 1000;
            total += d;
            json!({"job_id": u(j, "id"), "duration_ms": d})
        })
        .collect();
    let run_ms = (unix(s(run, "updated_at")).unwrap_or(0)
        - unix(s(run, "run_started_at"))
            .or_else(|| unix(s(run, "created_at")))
            .unwrap_or(0))
    .max(0)
        * 1000;
    json!({
        "billable": {"UBUNTU": {"total_ms": total, "jobs": jobs.len(), "job_runs": job_runs}},
        "run_duration_ms": run_ms,
    })
}

fn dispatch(ctx: &mut Ctx, repo: &str, id: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(wf) = find_workflow(ctx.svc, repo, id) else {
        return not_found();
    };
    if let Some(triggers) = wf.get("triggers").and_then(Value::as_array)
        && !triggers
            .iter()
            .any(|t| t.as_str() == Some("workflow_dispatch"))
    {
        return unprocessable("Workflow does not have 'workflow_dispatch' trigger");
    }
    if s(&wf, "state") == "disabled_manually" {
        return unprocessable("Cannot trigger a 'workflow_dispatch' on a disabled workflow");
    }
    let reference = s(&body, "ref").to_owned();
    if reference.is_empty() {
        return validation("WorkflowDispatch", "ref", "missing_field");
    }
    let Some(sha) = git::resolve_ref(ctx.svc, repo, &reference) else {
        return unprocessable(&format!("No ref found for: {reference}"));
    };
    let run_id = next_id(ctx.svc, "workflow_runs", "id", 1);
    let run_number = all(ctx.svc, "workflow_runs")
        .iter()
        .filter(|(_, r)| u(r, "workflow_id") == u(&wf, "id"))
        .map(|(_, r)| u(r, "run_number"))
        .max()
        .unwrap_or(0)
        + 1;
    let branch = reference
        .strip_prefix("refs/heads/")
        .unwrap_or(&reference)
        .to_owned();
    let doc = json!({
        "id": run_id,
        "repo": repo,
        "workflow_id": u(&wf, "id"),
        "name": s(&wf, "name"),
        "head_branch": branch,
        "head_sha": sha,
        "event": "workflow_dispatch",
        "status": "queued",
        "conclusion": null,
        "run_number": run_number,
        "run_attempt": 1,
        "actor": ctx.viewer(),
        "inputs": body.get("inputs").cloned().unwrap_or(json!({})),
        "created_at": ctx.now,
        "updated_at": ctx.now,
        "run_started_at": ctx.now,
    });
    ctx.svc
        .store
        .collection_mut("workflow_runs")
        .insert(run_id.to_string(), doc);
    no_content()
}

fn rerun(ctx: &mut Ctx, repo: &str, id: &str, failed_only: bool) -> Response {
    let Some(key) = run_key(ctx.svc, repo, id) else {
        return not_found();
    };
    let run = get(ctx.svc, "workflow_runs", &key)
        .cloned()
        .unwrap_or_default();
    if s(&run, "status") != "completed" {
        return error(StatusCode::FORBIDDEN, "This workflow is already running");
    }
    if failed_only
        && !matches!(
            run.get("conclusion").and_then(Value::as_str),
            Some("failure" | "cancelled" | "timed_out")
        )
    {
        return error(
            StatusCode::FORBIDDEN,
            "This workflow run has no failed jobs to re-run",
        );
    }
    let attempt = u(&run, "run_attempt").max(1) + 1;
    let now = ctx.now.clone();
    let viewer = ctx.viewer();
    let run_id = u(&run, "id");
    if let Some(d) = ctx.svc.store.collection_mut("workflow_runs").get_mut(&key) {
        d["status"] = json!("queued");
        d["conclusion"] = Value::Null;
        d["run_attempt"] = json!(attempt);
        d["updated_at"] = json!(now);
        d["run_started_at"] = json!(now);
        d["triggering_actor"] = json!(viewer);
        d["rerun_failed_only"] = json!(failed_only);
    }
    // Queue jobs for the new attempt.
    let jobs = jobs_of_run(ctx.svc, repo, run_id, true);
    for j in jobs {
        let failed = matches!(
            j.get("conclusion").and_then(Value::as_str),
            Some("failure" | "cancelled" | "timed_out")
        );
        if failed_only && !failed {
            continue;
        }
        let jid = next_id(ctx.svc, "workflow_jobs", "id", 1);
        let mut nj = j.clone();
        nj["id"] = json!(jid);
        nj["run_attempt"] = json!(attempt);
        nj["status"] = json!("queued");
        nj["conclusion"] = Value::Null;
        nj["started_at"] = json!(now);
        nj["completed_at"] = Value::Null;
        nj["log"] = Value::Null;
        if let Some(steps) = nj.get_mut("steps").and_then(Value::as_array_mut) {
            for st in steps {
                st["status"] = json!("queued");
                st["conclusion"] = Value::Null;
            }
        }
        ctx.svc
            .store
            .collection_mut("workflow_jobs")
            .insert(jid.to_string(), nj);
    }
    status_json(StatusCode::CREATED, json!({}))
}

fn cancel(ctx: &mut Ctx, repo: &str, id: &str) -> Response {
    let Some(key) = run_key(ctx.svc, repo, id) else {
        return not_found();
    };
    let run = get(ctx.svc, "workflow_runs", &key)
        .cloned()
        .unwrap_or_default();
    if s(&run, "status") == "completed" {
        return error(
            StatusCode::CONFLICT,
            "Cannot cancel a workflow run that is completed.",
        );
    }
    let now = ctx.now.clone();
    if let Some(d) = ctx.svc.store.collection_mut("workflow_runs").get_mut(&key) {
        d["status"] = json!("completed");
        d["conclusion"] = json!("cancelled");
        d["updated_at"] = json!(now);
    }
    let run_id = u(&run, "id");
    let job_keys: Vec<String> = all(ctx.svc, "workflow_jobs")
        .iter()
        .filter(|(_, j)| u(j, "run_id") == run_id && s(j, "status") != "completed")
        .map(|(k, _)| (*k).to_owned())
        .collect();
    for k in job_keys {
        if let Some(j) = ctx.svc.store.collection_mut("workflow_jobs").get_mut(&k) {
            j["status"] = json!("completed");
            j["conclusion"] = json!("cancelled");
            j["completed_at"] = json!(now);
        }
    }
    status_json(StatusCode::ACCEPTED, json!({}))
}

// ---------------------------------------------------------------- jobs

fn jobs_of_run(svc: &ServiceState, repo: &str, run_id: u64, latest_only: bool) -> Vec<Value> {
    let mut jobs: Vec<Value> = all(svc, "workflow_jobs")
        .into_iter()
        .map(|(_, j)| j.clone())
        .filter(|j| u(j, "run_id") == run_id && (s(j, "repo") == repo || s(j, "repo").is_empty()))
        .collect();
    jobs.sort_by_key(|j| u(j, "id"));
    if latest_only {
        // Latest attempt per job name.
        let mut latest: Vec<Value> = Vec::new();
        for j in jobs.into_iter().rev() {
            if !latest.iter().any(|l| s(l, "name") == s(&j, "name")) {
                latest.push(j);
            }
        }
        latest.reverse();
        return latest;
    }
    jobs
}

fn job_doc(svc: &ServiceState, repo: &str, id: &str) -> Option<Value> {
    get(svc, "workflow_jobs", id)
        .filter(|j| s(j, "repo") == repo || s(j, "repo").is_empty())
        .cloned()
        .filter(|j| {
            get(svc, "workflow_runs", &u(j, "run_id").to_string())
                .is_none_or(|r| s(r, "repo") == repo)
        })
}

fn job_json(ctx: &Ctx, j: &Value) -> Value {
    let id = u(j, "id");
    let run_id = u(j, "run_id");
    let run = get(ctx.svc, "workflow_runs", &run_id.to_string())
        .cloned()
        .unwrap_or_default();
    let repo = opt_s(j, "repo").unwrap_or_else(|| s(&run, "repo").to_owned());
    let api = format!("{}/repos/{repo}", ctx.req.api());
    let steps: Vec<Value> = arr(j, "steps")
        .iter()
        .enumerate()
        .map(|(i, st)| {
            json!({
                "name": s(st, "name"),
                "status": opt_s(st, "status").unwrap_or_else(|| s(j, "status").to_owned()),
                "conclusion": st.get("conclusion").cloned().unwrap_or(Value::Null),
                "number": st.get("number").cloned().unwrap_or(json!(i + 1)),
                "started_at": st.get("started_at").cloned().or_else(|| j.get("started_at").cloned()).unwrap_or(Value::Null),
                "completed_at": st.get("completed_at").cloned().or_else(|| j.get("completed_at").cloned()).unwrap_or(Value::Null),
            })
        })
        .collect();
    let wf_name = all(ctx.svc, "workflows")
        .into_iter()
        .find(|(_, w)| u(w, "id") == u(&run, "workflow_id"))
        .map(|(_, w)| s(w, "name").to_owned())
        .unwrap_or_else(|| s(&run, "name").to_owned());
    json!({
        "id": id,
        "run_id": run_id,
        "workflow_name": wf_name,
        "head_branch": run.get("head_branch").cloned().unwrap_or(Value::Null),
        "run_url": format!("{api}/actions/runs/{run_id}"),
        "run_attempt": u(j, "run_attempt").max(1),
        "node_id": opt_s(j, "node_id").unwrap_or_else(|| node_id("CR", &id.to_string())),
        "head_sha": s(&run, "head_sha"),
        "url": format!("{api}/actions/jobs/{id}"),
        "html_url": format!("{}/{repo}/actions/runs/{run_id}/job/{id}", ctx.base()),
        "status": opt_s(j, "status").unwrap_or_else(|| "completed".into()),
        "conclusion": j.get("conclusion").cloned().unwrap_or(Value::Null),
        "created_at": j.get("created_at").cloned().or_else(|| j.get("started_at").cloned()).unwrap_or(Value::Null),
        "started_at": j.get("started_at").cloned().unwrap_or(Value::Null),
        "completed_at": j.get("completed_at").cloned().unwrap_or(Value::Null),
        "name": s(j, "name"),
        "steps": steps,
        "check_run_url": format!("{api}/check-runs/{id}"),
        "labels": j.get("labels").cloned().unwrap_or(json!(["ubuntu-latest"])),
        "runner_id": u(j, "runner_id"),
        "runner_name": j.get("runner_name").cloned().unwrap_or(json!("ghes-runner-01")),
        "runner_group_id": 1,
        "runner_group_name": "Default",
    })
}

fn list_jobs(ctx: &Ctx, repo: &str, id: &str) -> Response {
    let Some(run) = run_doc(ctx.svc, repo, id) else {
        return not_found();
    };
    let latest = ctx.req.q("filter").unwrap_or("latest") != "all";
    let jobs: Vec<Value> = jobs_of_run(ctx.svc, repo, u(&run, "id"), latest)
        .iter()
        .map(|j| job_json(ctx, j))
        .collect();
    paged_wrapped(ctx.req, "jobs", jobs)
}

fn list_jobs_attempt(ctx: &Ctx, repo: &str, id: &str, attempt: &str) -> Response {
    let Some(run) = run_doc(ctx.svc, repo, id) else {
        return not_found();
    };
    let jobs: Vec<Value> = jobs_of_run(ctx.svc, repo, u(&run, "id"), false)
        .iter()
        .filter(|j| u(j, "run_attempt").max(1).to_string() == attempt)
        .map(|j| job_json(ctx, j))
        .collect();
    paged_wrapped(ctx.req, "jobs", jobs)
}

/// Job log text: the dataset's `log`, or one synthesized from the steps.
fn job_log(svc: &ServiceState, j: &Value) -> String {
    if let Some(log) = j.get("log").and_then(Value::as_str) {
        return log.to_owned();
    }
    let start = opt_s(j, "started_at").unwrap_or_default();
    let mut t = 0;
    let mut out = String::new();
    let stamp = |t: i64| {
        let ts = add_secs(&start, t);
        format!("{}.0000000Z", ts.trim_end_matches('Z'))
    };
    out.push_str(&format!("{} Current runner version: '2.321.0'\n", stamp(t)));
    for st in arr(j, "steps") {
        t += 1;
        out.push_str(&format!("{} ##[group]Run {}\n", stamp(t), s(st, "name")));
        out.push_str(&format!("{} ##[endgroup]\n", stamp(t)));
        if s(st, "conclusion") == "failure" {
            out.push_str(&format!(
                "{} ##[error]Process completed with exit code 1.\n",
                stamp(t)
            ));
        }
    }
    let _ = svc;
    out.push_str(&format!("{} Cleaning up orphan processes\n", stamp(t + 1)));
    out
}

// ---------------------------------------------------------------- artifacts

fn artifact_doc(svc: &ServiceState, repo: &str, id: &str) -> Option<Value> {
    get(svc, "artifacts", id)
        .filter(|a| s(a, "repo") == repo)
        .cloned()
}

fn artifact_json(ctx: &Ctx, a: &Value) -> Value {
    let repo = s(a, "repo");
    let id = u(a, "id");
    let run_id = u(a, "run_id");
    let run = get(ctx.svc, "workflow_runs", &run_id.to_string())
        .cloned()
        .unwrap_or_default();
    let api = format!("{}/repos/{repo}", ctx.req.api());
    json!({
        "id": id,
        "node_id": node_id("MDg", &id.to_string()),
        "name": s(a, "name"),
        "size_in_bytes": u(a, "size_in_bytes"),
        "url": format!("{api}/actions/artifacts/{id}"),
        "archive_download_url": format!("{api}/actions/artifacts/{id}/zip"),
        "expired": a.get("expired").cloned().unwrap_or(json!(false)),
        "digest": a.get("digest").cloned().unwrap_or(Value::Null),
        "created_at": a.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": a.get("updated_at").cloned().or_else(|| a.get("created_at").cloned()).unwrap_or(Value::Null),
        "expires_at": a.get("expires_at").cloned().unwrap_or(Value::Null),
        "workflow_run": {
            "id": run_id,
            "repository_id": get(ctx.svc, "repos", repo).map(|r| u(r, "id")).unwrap_or(0),
            "head_repository_id": get(ctx.svc, "repos", repo).map(|r| u(r, "id")).unwrap_or(0),
            "head_branch": run.get("head_branch").cloned().unwrap_or(Value::Null),
            "head_sha": run.get("head_sha").cloned().unwrap_or(Value::Null),
        },
    })
}

// ---------------------------------------------------------------- statuses / checks

fn statuses_for(svc: &ServiceState, repo: &str, sha: &str) -> Vec<Value> {
    let mut v: Vec<Value> = all(svc, "statuses")
        .into_iter()
        .map(|(_, st)| st.clone())
        .filter(|st| s(st, "repo") == repo && s(st, "sha") == sha)
        .collect();
    v.sort_by_key(|st| std::cmp::Reverse(u(st, "id")));
    v
}

fn status_json_doc(ctx: &Ctx, st: &Value) -> Value {
    let id = u(st, "id");
    let repo = s(st, "repo");
    let sha = s(st, "sha");
    json!({
        "url": format!("{}/repos/{repo}/statuses/{sha}", ctx.req.api()),
        "avatar_url": null,
        "id": id,
        "node_id": node_id("SC", &id.to_string()),
        "state": s(st, "state"),
        "description": st.get("description").cloned().unwrap_or(Value::Null),
        "target_url": st.get("target_url").cloned().unwrap_or(Value::Null),
        "context": opt_s(st, "context").unwrap_or_else(|| "default".into()),
        "created_at": st.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": st.get("updated_at").cloned().or_else(|| st.get("created_at").cloned()).unwrap_or(Value::Null),
        "creator": st.get("creator").map(|c| user_ref(ctx.svc, ctx.base(), c)).unwrap_or(Value::Null),
    })
}

fn list_statuses(ctx: &Ctx, repo: &str, reference: &str) -> Response {
    let Some(sha) = git::resolve_ref(ctx.svc, repo, reference) else {
        return not_found();
    };
    let out: Vec<Value> = statuses_for(ctx.svc, repo, &sha)
        .iter()
        .map(|st| status_json_doc(ctx, st))
        .collect();
    crate::util::paged(ctx.req, out)
}

fn combined_status(ctx: &Ctx, repo: &str, reference: &str) -> Response {
    let Some(sha) = git::resolve_ref(ctx.svc, repo, reference) else {
        return not_found();
    };
    // Latest status per context.
    let mut latest: Vec<Value> = Vec::new();
    for st in statuses_for(ctx.svc, repo, &sha) {
        if !latest.iter().any(|l| s(l, "context") == s(&st, "context")) {
            latest.push(st);
        }
    }
    latest.reverse();
    let state = if latest.is_empty() {
        "pending"
    } else if latest
        .iter()
        .any(|st| matches!(s(st, "state"), "failure" | "error"))
    {
        "failure"
    } else if latest.iter().any(|st| s(st, "state") == "pending") {
        "pending"
    } else {
        "success"
    };
    let statuses: Vec<Value> = latest.iter().map(|st| status_json_doc(ctx, st)).collect();
    ok(json!({
        "state": state,
        "statuses": statuses,
        "sha": sha,
        "total_count": statuses.len(),
        "repository": render::repo_min(ctx.svc, ctx.base(), repo),
        "commit_url": format!("{}/repos/{repo}/commits/{sha}", ctx.req.api()),
        "url": format!("{}/repos/{repo}/commits/{sha}/status", ctx.req.api()),
    }))
}

fn create_status(ctx: &mut Ctx, repo: &str, sha: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let state = s(&body, "state").to_owned();
    if !["error", "failure", "pending", "success"].contains(&state.as_str()) {
        return validation("Status", "state", "invalid");
    }
    if git::commit(ctx.svc, repo, sha).is_none() {
        return unprocessable(&format!("No commit found for SHA: {sha}"));
    }
    let id = next_id(ctx.svc, "statuses", "id", 1);
    let doc = json!({
        "id": id,
        "repo": repo,
        "sha": sha,
        "state": state,
        "context": opt_s(&body, "context").unwrap_or_else(|| "default".into()),
        "description": body.get("description").cloned().unwrap_or(Value::Null),
        "target_url": body.get("target_url").cloned().unwrap_or(Value::Null),
        "creator": ctx.viewer(),
        "created_at": ctx.now,
    });
    ctx.svc
        .store
        .collection_mut("statuses")
        .insert(id.to_string(), doc.clone());
    created(status_json_doc(ctx, &doc))
}

/// Explicit check runs plus one check run per Actions job (as GitHub does).
fn all_check_runs(svc: &ServiceState, repo: &str) -> Vec<Value> {
    let mut out: Vec<Value> = all(svc, "check_runs")
        .into_iter()
        .map(|(_, c)| c.clone())
        .filter(|c| s(c, "repo") == repo)
        .collect();
    for (_, run) in all(svc, "workflow_runs") {
        if s(run, "repo") != repo {
            continue;
        }
        for j in jobs_of_run(svc, repo, u(run, "id"), true) {
            let failed_step = arr(&j, "steps")
                .iter()
                .find(|st| s(st, "conclusion") == "failure")
                .map(|st| s(st, "name").to_owned());
            out.push(json!({
                "id": u(&j, "id"),
                "repo": repo,
                "head_sha": s(run, "head_sha"),
                "name": s(&j, "name"),
                "status": opt_s(&j, "status").unwrap_or_else(|| "completed".into()),
                "conclusion": j.get("conclusion").cloned().unwrap_or(Value::Null),
                "started_at": j.get("started_at").cloned().unwrap_or(Value::Null),
                "completed_at": j.get("completed_at").cloned().unwrap_or(Value::Null),
                "output": {
                    "title": failed_step.as_ref().map(|st| format!("Step '{st}' failed")),
                    "summary": failed_step.as_ref().map(|_| "Process completed with exit code 1.".to_owned()),
                    "text": null,
                    "annotations_count": usize::from(failed_step.is_some()),
                },
                "details_url": format!("/actions/runs/{}/job/{}", u(run, "id"), u(&j, "id")),
                "app": "github-actions",
            }));
        }
    }
    out
}

fn check_run_json(ctx: &Ctx, repo: &str, c: &Value) -> Value {
    let id = u(c, "id");
    let api = format!("{}/repos/{repo}", ctx.req.api());
    let app = opt_s(c, "app").unwrap_or_else(|| "github-actions".into());
    let details = opt_s(c, "details_url")
        .map(|d| {
            if d.starts_with('/') {
                format!("{}/{repo}{d}", ctx.base())
            } else {
                d
            }
        })
        .unwrap_or_else(|| format!("{}/{repo}/runs/{id}", ctx.base()));
    let output = c.get("output").cloned().unwrap_or(json!({}));
    json!({
        "id": id,
        "head_sha": s(c, "head_sha"),
        "node_id": node_id("CR", &id.to_string()),
        "external_id": "",
        "url": format!("{api}/check-runs/{id}"),
        "html_url": format!("{}/{repo}/runs/{id}", ctx.base()),
        "details_url": details,
        "status": opt_s(c, "status").unwrap_or_else(|| "completed".into()),
        "conclusion": c.get("conclusion").cloned().unwrap_or(Value::Null),
        "started_at": c.get("started_at").cloned().unwrap_or(Value::Null),
        "completed_at": c.get("completed_at").cloned().unwrap_or(Value::Null),
        "output": {
            "title": output.get("title").cloned().unwrap_or(Value::Null),
            "summary": output.get("summary").cloned().unwrap_or(Value::Null),
            "text": output.get("text").cloned().unwrap_or(Value::Null),
            "annotations_count": output.get("annotations_count").cloned().unwrap_or(json!(0)),
            "annotations_url": format!("{api}/check-runs/{id}/annotations"),
        },
        "name": s(c, "name"),
        "check_suite": {"id": id / 10 + 1},
        "app": {"id": 15368, "slug": app, "name": if app == "github-actions" { "GitHub Actions".to_owned() } else { app.clone() }, "owner": null},
        "pull_requests": [],
    })
}

fn check_runs_for_ref(ctx: &Ctx, repo: &str, reference: &str) -> Response {
    let Some(sha) = git::resolve_ref(ctx.svc, repo, reference) else {
        return unprocessable(&format!("No commit found for SHA: {reference}"));
    };
    let name = ctx.req.q("check_name");
    let status = ctx.req.q("status");
    let runs: Vec<Value> = all_check_runs(ctx.svc, repo)
        .iter()
        .filter(|c| s(c, "head_sha") == sha)
        .filter(|c| name.is_none_or(|n| s(c, "name") == n))
        .filter(|c| status.is_none_or(|st| s(c, "status") == st))
        .map(|c| check_run_json(ctx, repo, c))
        .collect();
    paged_wrapped(ctx.req, "check_runs", runs)
}

fn commit_pulls(ctx: &Ctx, repo: &str, reference: &str) -> Response {
    let Some(sha) = git::resolve_ref(ctx.svc, repo, reference) else {
        return unprocessable(&format!("No commit found for SHA: {reference}"));
    };
    let out: Vec<Value> = with_prefix(ctx.svc, "pulls", &format!("{repo}#"))
        .iter()
        .filter(|(_, p)| {
            let (_, commits) = render::pr_changes(ctx.svc, repo, p);
            commits.iter().any(|c| s(c, "sha") == sha) || s(p, "merge_commit_sha") == sha
        })
        .map(|(k, _)| render::pr_list_json(ctx.svc, ctx.base(), k))
        .collect();
    crate::util::paged(ctx.req, out)
}
