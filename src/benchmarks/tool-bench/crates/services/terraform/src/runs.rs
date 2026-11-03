//! Runs and their plans, applies, comments, policy checks, plan exports, and the archivist
//! object store that serves logs, JSON plans, state files and export bundles.

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mock_core::world::{SharedWorld, WorldState};
use serde_json::{Value, json};

use crate::{
    NAME,
    common::{
        Ctx, Db, attributes, body_json, conflict, data, data_with_included, list_response, new_id,
        not_found, opt_str, redirect, relationship_id, str_field,
    },
    resources::{self, CANCELABLE, CONFIRMABLE},
    tarball,
};

pub fn routes() -> Router<SharedWorld> {
    Router::new()
        .route("/api/v2/runs", post(create_run))
        .route("/api/v2/runs/{id}", get(get_run))
        .route("/api/v2/runs/{id}/actions/{action}", post(run_action))
        .route(
            "/api/v2/runs/{id}/comments",
            get(list_comments).post(create_comment),
        )
        .route("/api/v2/comments/{id}", get(get_comment))
        .route("/api/v2/runs/{id}/plan", get(run_plan))
        .route("/api/v2/runs/{id}/apply", get(run_apply))
        .route("/api/v2/runs/{id}/policy-checks", get(list_policy_checks))
        .route("/api/v2/policy-checks/{id}", get(get_policy_check))
        .route("/api/v2/workspaces/{id}/runs", get(list_workspace_runs))
        .route("/api/v2/organizations/{org}/runs", get(list_org_runs))
        .route("/api/v2/plans/{id}", get(get_plan))
        .route("/api/v2/plans/{id}/json-output", get(plan_json_output))
        .route(
            "/api/v2/plans/{id}/json-output-redacted",
            get(plan_json_output),
        )
        .route("/api/v2/applies/{id}", get(get_apply))
        .route("/api/v2/plan-exports", post(create_plan_export))
        .route(
            "/api/v2/plan-exports/{id}",
            get(get_plan_export).delete(delete_plan_export),
        )
        .route(
            "/api/v2/plan-exports/{id}/download",
            get(download_plan_export),
        )
}

pub fn archivist_routes() -> Router<SharedWorld> {
    Router::new().route("/archivist/v1/object/{key}", get(archivist_object))
}

fn run_included(db: &Db, run: &Value, ctx: &Ctx) -> Vec<Value> {
    let mut included = Vec::new();
    let q = &ctx.q;
    if q.has_include("workspace")
        && let Some(ws) = opt_str(run, "workspace").and_then(|w| db.get("workspaces", w))
    {
        included.push(resources::workspace(db, ws));
    }
    if q.has_include("plan")
        && let Some(p) = opt_str(run, "plan").and_then(|p| db.get("plans", p))
    {
        included.push(resources::plan(db, &ctx.base, p));
    }
    if q.has_include("apply")
        && let Some(a) = opt_str(run, "apply").and_then(|a| db.get("applies", a))
    {
        included.push(resources::apply(&ctx.base, a));
    }
    if q.has_include("created_by")
        && let Some(u) = opt_str(run, "created-by").and_then(|u| db.get("users", u))
    {
        included.push(resources::user(u));
    }
    included
}

fn sorted_runs(mut runs: Vec<&Value>) -> Vec<&Value> {
    runs.sort_by(|a, b| {
        str_field(b, "created-at")
            .cmp(str_field(a, "created-at"))
            .then_with(|| str_field(b, "id").cmp(str_field(a, "id")))
    });
    runs
}

fn run_matches(db: &Db, run: &Value, ctx: &Ctx) -> bool {
    let q = &ctx.q;
    let statuses = q.list("filter[status]");
    let sources = q.list("filter[source]");
    let operations = q.list("filter[operation]");
    if !statuses.is_empty() && !statuses.iter().any(|s| s == str_field(run, "status")) {
        return false;
    }
    if !sources.is_empty() && !sources.iter().any(|s| s == str_field(run, "source")) {
        return false;
    }
    if !operations.is_empty() {
        let op = if run.get("is-destroy").and_then(Value::as_bool) == Some(true) {
            "destroy"
        } else if run.get("plan-only").and_then(Value::as_bool) == Some(true) {
            "plan_only"
        } else if run.get("refresh-only").and_then(Value::as_bool) == Some(true) {
            "refresh_only"
        } else {
            "plan_and_apply"
        };
        if !operations.iter().any(|o| o == op) {
            return false;
        }
    }
    let username = opt_str(run, "created-by")
        .and_then(|u| db.get("users", u))
        .map(|u| str_field(u, "username").to_owned())
        .unwrap_or_default();
    if let Some(user) = q.value("search[user]")
        && !username.eq_ignore_ascii_case(user)
        && !str_field(run, "vcs-username").eq_ignore_ascii_case(user)
    {
        return false;
    }
    if let Some(commit) = q.value("search[commit]")
        && !str_field(run, "commit-sha").starts_with(commit)
    {
        return false;
    }
    if q.value("search[user]").is_none()
        && q.value("search[commit]").is_none()
        && let Some(basic) = q.value("search[basic]")
    {
        let basic = basic.to_lowercase();
        let hay = format!(
            "{} {} {} {}",
            str_field(run, "id"),
            str_field(run, "message"),
            username,
            str_field(run, "commit-sha")
        )
        .to_lowercase();
        if !hay.contains(&basic) {
            return false;
        }
    }
    true
}

async fn list_workspace_runs(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    if db.get("workspaces", &id).is_none() {
        return not_found();
    }
    let runs = sorted_runs(db.filter("runs", |r| {
        str_field(r, "workspace") == id && run_matches(&db, r, &ctx)
    }));
    let mut included = Vec::new();
    let items = runs
        .into_iter()
        .map(|r| {
            included.extend(run_included(&db, r, &ctx));
            resources::run(&db, r)
        })
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/workspaces/{id}/runs"),
        &ctx.q,
        items,
        included,
    )
}

async fn list_org_runs(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(o) = db.org(&org) else {
        return not_found();
    };
    let org_name = str_field(o, "name");
    let ws_names = ctx.q.list("filter[workspace_names]");
    let runs = sorted_runs(db.filter("runs", |r| {
        let Some(ws) = opt_str(r, "workspace").and_then(|w| db.get("workspaces", w)) else {
            return false;
        };
        str_field(ws, "organization") == org_name
            && (ws_names.is_empty() || ws_names.iter().any(|n| n == str_field(ws, "name")))
            && run_matches(&db, r, &ctx)
    }));
    let mut included = Vec::new();
    let items = runs
        .into_iter()
        .map(|r| {
            included.extend(run_included(&db, r, &ctx));
            resources::run(&db, r)
        })
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/runs"),
        &ctx.q,
        items,
        included,
    )
}

async fn get_run(State(world): State<SharedWorld>, Path(id): Path<String>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.get("runs", &id) {
        Some(r) => data_with_included(resources::run(&db, r), run_included(&db, r, &ctx)),
        None => not_found(),
    }
}

/// Resource addresses touched by a plan, used to synthesize logs and the JSON plan.
fn changes_of(plan: &Value) -> Vec<Value> {
    plan.get("resource-changes-detail")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn count_actions(changes: &[Value]) -> (u64, u64, u64) {
    let (mut add, mut change, mut destroy) = (0, 0, 0);
    for c in changes {
        let actions: Vec<&str> = c
            .get("actions")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if actions.contains(&"create") {
            add += 1;
        }
        if actions.contains(&"delete") {
            destroy += 1;
        }
        if actions == ["update"] {
            change += 1;
        }
    }
    (add, change, destroy)
}

/// Creates a run and finishes its plan immediately (the mock has no execution delay).
async fn create_run(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let Some(ws_id) = relationship_id(&doc, "workspace") else {
        return crate::common::invalid("Workspace is required", "workspace");
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let (ws, sv, vars, user) = {
        let db = Db::new(world);
        let Some(ws) = db.get("workspaces", &ws_id).cloned() else {
            return not_found();
        };
        let sv = opt_str(&ws, "current-state-version")
            .and_then(|s| db.get("state_versions", s))
            .cloned();
        let vars: serde_json::Map<String, Value> = db
            .filter("vars", |v| str_field(v, "workspace") == ws_id)
            .iter()
            .map(|v| {
                let value = if v.get("sensitive").and_then(Value::as_bool) == Some(true) {
                    json!("(sensitive value)")
                } else {
                    v.get("value").cloned().unwrap_or(json!(""))
                };
                (str_field(v, "key").to_owned(), value)
            })
            .collect();
        let user = db.current_user().map(|u| str_field(u, "id").to_owned());
        (ws, sv, vars, user)
    };
    let flag = |name: &str| attrs.get(name).and_then(Value::as_bool);
    let is_destroy = flag("is-destroy").unwrap_or(false);
    let plan_only = flag("plan-only").unwrap_or(false);
    let refresh_only = flag("refresh-only").unwrap_or(false);
    let allow_empty = flag("allow-empty-apply").unwrap_or(false);
    let auto_apply = flag("auto-apply").unwrap_or_else(|| {
        ws.get("auto-apply")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });
    let message = attrs
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Queued manually using the Terraform API")
        .to_owned();
    let tf_version = attrs
        .get("terraform-version")
        .and_then(Value::as_str)
        .unwrap_or(str_field(&ws, "terraform-version"))
        .to_owned();
    let tf_version = if tf_version.is_empty() {
        "1.9.5".to_owned()
    } else {
        tf_version
    };

    let changes: Vec<Value> = if refresh_only {
        Vec::new()
    } else if is_destroy {
        sv.as_ref()
            .and_then(|sv| sv.get("resources"))
            .and_then(Value::as_array)
            .map(|res| {
                res.iter()
                    .flat_map(|r| {
                        let count = r.get("count").and_then(Value::as_u64).unwrap_or(1);
                        let module = str_field(r, "module");
                        let prefix = if module.is_empty() || module == "root" {
                            String::new()
                        } else {
                            format!("{module}.")
                        };
                        let base =
                            format!("{prefix}{}.{}", str_field(r, "type"), str_field(r, "name"));
                        (0..count).map(move |i| {
                            let address = if count > 1 {
                                format!("{base}[{i}]")
                            } else {
                                base.clone()
                            };
                            json!({ "address": address, "actions": ["delete"] })
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        ws.pointer("/simulated-plan/resource-changes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let (add, change, destroy) = count_actions(&changes);
    let has_changes = add + change + destroy > 0;

    let now = world.clock.now_rfc3339();
    let run_id = new_id(world, "runs", "run", "run-");
    let plan_id = new_id(world, "plans", "plan", "plan-");
    let apply_id = new_id(world, "applies", "apply", "apply-");

    let status = if plan_only || (!has_changes && !allow_empty) {
        "planned_and_finished"
    } else if auto_apply {
        "applied"
    } else {
        "planned"
    };
    let mut timestamps = json!({
        "plan-queueable-at": now,
        "plan-queued-at": now,
        "planning-at": now,
        "planned-at": now,
    });
    if status == "planned_and_finished" {
        timestamps["planned-and-finished-at"] = json!(now);
    }
    if status == "applied" {
        timestamps["confirmed-at"] = json!(now);
        timestamps["apply-queued-at"] = json!(now);
        timestamps["applying-at"] = json!(now);
        timestamps["applied-at"] = json!(now);
    }
    let run = json!({
        "id": run_id,
        "workspace": ws_id,
        "status": status,
        "message": message,
        "source": "tfe-api",
        "trigger-reason": "manual",
        "created-at": now,
        "has-changes": has_changes,
        "is-destroy": is_destroy,
        "plan-only": plan_only,
        "refresh-only": refresh_only,
        "refresh": flag("refresh").unwrap_or(true),
        "auto-apply": auto_apply,
        "allow-empty-apply": allow_empty,
        "save-plan": flag("save-plan").unwrap_or(false),
        "terraform-version": tf_version,
        "target-addrs": attrs.get("target-addrs").cloned().unwrap_or(json!([])),
        "replace-addrs": attrs.get("replace-addrs").cloned().unwrap_or(json!([])),
        "variables": attrs.get("variables").cloned().unwrap_or(json!([])),
        "status-timestamps": timestamps,
        "plan": plan_id,
        "apply": apply_id,
        "created-by": user,
        "confirmed-by": if status == "applied" { json!(user) } else { Value::Null },
        "input-variables": vars,
    });
    let plan = json!({
        "id": plan_id,
        "run": run_id,
        "status": "finished",
        "has-changes": has_changes,
        "resource-additions": add,
        "resource-changes": change,
        "resource-destructions": destroy,
        "resource-imports": 0,
        "status-timestamps": { "queued-at": now, "pending-at": now, "started-at": now, "finished-at": now },
        "resource-changes-detail": changes,
    });
    let apply_status = match status {
        "applied" => "finished",
        "planned" => "pending",
        _ => "unreachable",
    };
    let mut apply = json!({
        "id": apply_id,
        "run": run_id,
        "status": apply_status,
        "resource-additions": if status == "applied" { add } else { 0 },
        "resource-changes": if status == "applied" { change } else { 0 },
        "resource-destructions": if status == "applied" { destroy } else { 0 },
        "resource-imports": 0,
        "status-timestamps": if status == "applied" {
            json!({ "queued-at": now, "started-at": now, "finished-at": now })
        } else {
            json!({})
        },
    });
    {
        let store = &mut world.service_mut(NAME).store;
        store.collection_mut("plans").insert(plan_id, plan);
        store
            .collection_mut("runs")
            .insert(run_id.clone(), run.clone());
    }
    if status == "applied" {
        let sv_id = finish_apply(world, &ws_id, &run_id, add, destroy, &now);
        apply["state-version"] = json!(sv_id);
    }
    world
        .service_mut(NAME)
        .store
        .collection_mut("applies")
        .insert(apply_id, apply);
    update_workspace_run_pointers(world, &ws_id, &run_id, plan_only, &now);
    let db = Db::new(world);
    let run = db.get("runs", &run_id).expect("run just created");
    data(StatusCode::CREATED, resources::run(&db, run))
}

/// Points the workspace at a new run and keeps a summary of it for graders and humans.
fn update_workspace_run_pointers(
    world: &mut WorldState,
    ws_id: &str,
    run_id: &str,
    speculative: bool,
    now: &str,
) {
    let run = Db::new(world)
        .get("runs", run_id)
        .cloned()
        .unwrap_or(Value::Null);
    let store = &mut world.service_mut(NAME).store;
    if let Some(ws) = store.collection_mut("workspaces").get_mut(ws_id) {
        if !speculative {
            ws["current-run"] = json!(run_id);
        }
        ws["latest-run"] = json!(run_id);
        ws["latest-run-summary"] = run_summary(&run);
        ws["updated-at"] = json!(now);
    }
}

fn run_summary(run: &Value) -> Value {
    json!({
        "id": run["id"],
        "status": run["status"],
        "message": run["message"],
        "is-destroy": run["is-destroy"],
        "plan-only": run["plan-only"],
        "refresh-only": run["refresh-only"],
        "auto-apply": run["auto-apply"],
        "created-at": run["created-at"],
        "input-variables": run.get("input-variables").cloned().unwrap_or(json!({})),
    })
}

/// Applies a run's changes to the workspace: resource count, a new state version, unlock.
/// Returns the new state version's ID.
fn finish_apply(
    world: &mut WorldState,
    ws_id: &str,
    run_id: &str,
    add: u64,
    destroy: u64,
    now: &str,
) -> String {
    let (prev, serial) = {
        let db = Db::new(world);
        let prev = db
            .get("workspaces", ws_id)
            .and_then(|w| opt_str(w, "current-state-version"))
            .and_then(|s| db.get("state_versions", s))
            .cloned();
        let serial = db
            .filter("state_versions", |s| str_field(s, "workspace") == ws_id)
            .iter()
            .filter_map(|s| s.get("serial").and_then(Value::as_u64))
            .max()
            .unwrap_or(0)
            + 1;
        (prev, serial)
    };
    let sv_id = new_id(world, "state_versions", "state_version", "sv-");
    let user = Db::new(world)
        .current_user()
        .map(|u| str_field(u, "id").to_owned());
    let mut sv = prev
        .clone()
        .unwrap_or_else(|| json!({ "outputs": [], "resources": [] }));
    sv["id"] = json!(sv_id);
    sv["workspace"] = json!(ws_id);
    sv["run"] = json!(run_id);
    sv["serial"] = json!(serial);
    sv["created-at"] = json!(now);
    sv["created-by"] = json!(user);
    if let Some(outputs) = sv.get_mut("outputs").and_then(Value::as_array_mut) {
        for (i, o) in outputs.iter_mut().enumerate() {
            o["id"] = json!(format!("wsout-{}{i}", &sv_id[3..15]));
        }
    }
    let is_destroy = Db::new(world)
        .get("runs", run_id)
        .and_then(|r| r.get("is-destroy"))
        .and_then(Value::as_bool)
        == Some(true);
    if is_destroy {
        sv["resources"] = json!([]);
        sv["outputs"] = json!([]);
    }
    let store = &mut world.service_mut(NAME).store;
    store
        .collection_mut("state_versions")
        .insert(sv_id.clone(), sv);
    if let Some(ws) = store.collection_mut("workspaces").get_mut(ws_id) {
        let count = ws
            .get("resource-count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        ws["resource-count"] = json!(if is_destroy {
            0
        } else {
            (count + add).saturating_sub(destroy)
        });
        ws["current-state-version"] = json!(sv_id);
        ws["current-run"] = json!(run_id);
        if ws.pointer("/locked-by/id").and_then(Value::as_str) == Some(run_id) {
            ws["locked"] = json!(false);
            ws["locked-by"] = Value::Null;
        }
    }
    sv_id
}

fn comment_text(body: &Bytes) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("comment").and_then(Value::as_str).map(str::to_owned))
}

async fn run_action(
    State(world): State<SharedWorld>,
    Path((id, action)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let comment = comment_text(&body);
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let Some(run) = Db::new(world).get("runs", &id).cloned() else {
        return not_found();
    };
    let status = str_field(&run, "status").to_owned();
    let ws_id = str_field(&run, "workspace").to_owned();
    let now = world.clock.now_rfc3339();
    let user = Db::new(world)
        .current_user()
        .map(|u| str_field(u, "id").to_owned());
    match action.as_str() {
        "apply" => {
            if !CONFIRMABLE.contains(&status.as_str()) {
                return conflict(
                    "transition not allowed",
                    "The run cannot be confirmed at this time.",
                );
            }
            let plan = opt_str(&run, "plan")
                .and_then(|p| Db::new(world).get("plans", p))
                .cloned()
                .unwrap_or(Value::Null);
            let num = |k: &str| plan.get(k).and_then(Value::as_u64).unwrap_or(0);
            let (add, change, destroy) = (
                num("resource-additions"),
                num("resource-changes"),
                num("resource-destructions"),
            );
            let sv_id = finish_apply(world, &ws_id, &id, add, destroy, &now);
            let store = &mut world.service_mut(NAME).store;
            if let Some(r) = store.collection_mut("runs").get_mut(&id) {
                r["status"] = json!("applied");
                r["confirmed-by"] = json!(user);
                r["confirm-comment"] = json!(comment);
                for k in [
                    "confirmed-at",
                    "apply-queued-at",
                    "applying-at",
                    "applied-at",
                ] {
                    r["status-timestamps"][k] = json!(now);
                }
            }
            if let Some(apply_id) = opt_str(&run, "apply")
                && let Some(a) = store.collection_mut("applies").get_mut(apply_id)
            {
                a["status"] = json!("finished");
                a["resource-additions"] = json!(add);
                a["resource-changes"] = json!(change);
                a["resource-destructions"] = json!(destroy);
                a["state-version"] = json!(sv_id);
                a["status-timestamps"] =
                    json!({ "queued-at": now, "started-at": now, "finished-at": now });
            }
        }
        "discard" => {
            if !CONFIRMABLE.contains(&status.as_str()) && status != "policy_soft_failed" {
                return conflict(
                    "transition not allowed",
                    "The run cannot be discarded at this time.",
                );
            }
            let store = &mut world.service_mut(NAME).store;
            if let Some(r) = store.collection_mut("runs").get_mut(&id) {
                r["status"] = json!("discarded");
                r["discard-comment"] = json!(comment);
                r["status-timestamps"]["discarded-at"] = json!(now);
            }
            if let Some(apply_id) = opt_str(&run, "apply")
                && let Some(a) = store.collection_mut("applies").get_mut(apply_id)
            {
                a["status"] = json!("unreachable");
            }
            release_run_lock(world, &ws_id, &id);
        }
        "cancel" | "force-cancel" => {
            if !CANCELABLE.contains(&status.as_str()) {
                return conflict(
                    "transition not allowed",
                    "The run cannot be canceled at this time.",
                );
            }
            let store = &mut world.service_mut(NAME).store;
            if let Some(r) = store.collection_mut("runs").get_mut(&id) {
                r["status"] = json!(if action == "cancel" {
                    "canceled"
                } else {
                    "force_canceled"
                });
                r["cancel-comment"] = json!(comment);
                r["canceled-at"] = json!(now);
                r["status-timestamps"]["canceled-at"] = json!(now);
            }
            for (collection, key) in [("plans", "plan"), ("applies", "apply")] {
                if let Some(child) = opt_str(&run, key)
                    && let Some(d) = store.collection_mut(collection).get_mut(child)
                    && !["finished", "errored", "unreachable"].contains(&str_field(d, "status"))
                {
                    d["status"] = json!(if key == "plan" {
                        "canceled"
                    } else {
                        "unreachable"
                    });
                }
            }
            release_run_lock(world, &ws_id, &id);
        }
        "force-execute" => {
            if status != "pending" {
                return conflict(
                    "transition not allowed",
                    "The run cannot be force executed at this time.",
                );
            }
        }
        _ => return not_found(),
    }
    let run = Db::new(world)
        .get("runs", &id)
        .cloned()
        .unwrap_or(Value::Null);
    let store = &mut world.service_mut(NAME).store;
    if let Some(ws) = store.collection_mut("workspaces").get_mut(&ws_id)
        && ws.pointer("/latest-run-summary/id").and_then(Value::as_str) == Some(id.as_str())
    {
        ws["latest-run-summary"] = run_summary(&run);
    }
    (
        StatusCode::ACCEPTED,
        [(header::CONTENT_TYPE, "application/vnd.api+json")],
        "",
    )
        .into_response()
}

fn release_run_lock(world: &mut WorldState, ws_id: &str, run_id: &str) {
    let store = &mut world.service_mut(NAME).store;
    if let Some(ws) = store.collection_mut("workspaces").get_mut(ws_id)
        && ws.pointer("/locked-by/id").and_then(Value::as_str) == Some(run_id)
    {
        ws["locked"] = json!(false);
        ws["locked-by"] = Value::Null;
    }
}

// ---------------------------------------------------------------- comments

async fn list_comments(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    if db.get("runs", &id).is_none() {
        return not_found();
    }
    let items = db
        .filter("comments", |c| str_field(c, "run") == id)
        .into_iter()
        .map(resources::comment)
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/runs/{id}/comments"),
        &ctx.q,
        items,
        vec![],
    )
}

async fn create_comment(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let text = attributes(&doc)
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if text.is_empty() {
        return crate::common::invalid("Body can't be blank", "body");
    }
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    if Db::new(world).get("runs", &id).is_none() {
        return not_found();
    }
    let now = world.clock.now_rfc3339();
    let user = Db::new(world)
        .current_user()
        .map(|u| str_field(u, "id").to_owned());
    let cid = new_id(world, "comments", "comment", "wsc-");
    let comment = json!({ "id": cid, "run": id, "body": text, "created-at": now, "user": user });
    world
        .service_mut(NAME)
        .store
        .collection_mut("comments")
        .insert(cid, comment.clone());
    data(StatusCode::CREATED, resources::comment(&comment))
}

async fn get_comment(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world).get("comments", &id) {
        Some(c) => data(StatusCode::OK, resources::comment(c)),
        None => not_found(),
    }
}

// ---------------------------------------------------------------- plans and applies

async fn get_plan(State(world): State<SharedWorld>, Path(id): Path<String>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.get("plans", &id) {
        Some(p) => data(StatusCode::OK, resources::plan(&db, &ctx.base, p)),
        None => not_found(),
    }
}

async fn run_plan(State(world): State<SharedWorld>, Path(id): Path<String>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db
        .get("runs", &id)
        .and_then(|r| opt_str(r, "plan"))
        .and_then(|p| db.get("plans", p))
    {
        Some(p) => data(StatusCode::OK, resources::plan(&db, &ctx.base, p)),
        None => not_found(),
    }
}

async fn get_apply(State(world): State<SharedWorld>, Path(id): Path<String>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world).get("applies", &id) {
        Some(a) => data(StatusCode::OK, resources::apply(&ctx.base, a)),
        None => not_found(),
    }
}

async fn run_apply(State(world): State<SharedWorld>, Path(id): Path<String>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db
        .get("runs", &id)
        .and_then(|r| opt_str(r, "apply"))
        .and_then(|a| db.get("applies", a))
    {
        Some(a) => data(StatusCode::OK, resources::apply(&ctx.base, a)),
        None => not_found(),
    }
}

/// JSON plans are served from the archivist via a temporary redirect, like HCP Terraform.
async fn plan_json_output(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    if db.get("plans", &id).is_none() {
        return not_found();
    }
    redirect(&format!("{}/archivist/v1/object/plan-json-{id}", ctx.base))
}

fn symbol(actions: &[&str]) -> (&'static str, &'static str) {
    match actions {
        ["create"] => ("+", "will be created"),
        ["delete"] => ("-", "will be destroyed"),
        ["update"] => ("~", "will be updated in-place"),
        ["delete", "create"] | ["create", "delete"] => ("-/+", "must be replaced"),
        ["read"] => ("<=", "will be read during apply"),
        _ => (" ", "has no changes"),
    }
}

fn render_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

/// Terraform-style plan log built from the plan's resource changes.
pub fn plan_log(db: &Db, plan: &Value) -> String {
    if let Some(log) = plan.get("log").and_then(Value::as_str) {
        return log.to_owned();
    }
    let run = opt_str(plan, "run").and_then(|r| db.get("runs", r));
    let tf = run
        .map(|r| str_field(r, "terraform-version").to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "1.9.5".to_owned());
    let mut out = format!("Terraform v{tf}\non linux_amd64\nInitializing plugins and modules...\n");
    let changes = changes_of(plan);
    if changes.is_empty() {
        out.push_str("\nNo changes. Your infrastructure matches the configuration.\n\nTerraform has compared your real infrastructure against your configuration\nand found no differences, so no changes are needed.\n");
        return out;
    }
    out.push_str("\nTerraform used the selected providers to generate the following execution\nplan. Resource actions are indicated with the following symbols:\n");
    let mut seen = Vec::new();
    for c in &changes {
        let actions: Vec<&str> = c["actions"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let (sym, _) = symbol(&actions);
        if !seen.contains(&sym) {
            seen.push(sym);
        }
    }
    for sym in &seen {
        let text = match *sym {
            "+" => "create",
            "-" => "destroy",
            "~" => "update in-place",
            "-/+" => "destroy and then create replacement",
            "<=" => "read (data resources)",
            _ => "no-op",
        };
        out.push_str(&format!("  {sym} {text}\n"));
    }
    out.push_str("\nTerraform will perform the following actions:\n");
    for c in &changes {
        let address = str_field(c, "address");
        let actions: Vec<&str> = c["actions"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let (sym, verb) = symbol(&actions);
        let (kind, name) = address
            .rsplit_once('.')
            .map(|(k, n)| (k.rsplit('.').next().unwrap_or(k), n))
            .unwrap_or((address, ""));
        out.push_str(&format!(
            "\n  # {address} {verb}\n  {sym} resource \"{kind}\" \"{name}\" {{\n"
        ));
        let before = c.get("before").and_then(Value::as_object);
        let after = c.get("after").and_then(Value::as_object);
        match (before, after) {
            (Some(b), Some(a)) => {
                for (k, v) in a {
                    match b.get(k) {
                        Some(old) if old != v => out.push_str(&format!(
                            "      ~ {k} = {} -> {}\n",
                            render_value(old),
                            render_value(v)
                        )),
                        Some(_) => {}
                        None => out.push_str(&format!("      + {k} = {}\n", render_value(v))),
                    }
                }
            }
            (None, Some(a)) => {
                for (k, v) in a {
                    out.push_str(&format!("      + {k} = {}\n", render_value(v)));
                }
            }
            (Some(b), None) => {
                for (k, v) in b {
                    out.push_str(&format!("      - {k} = {} -> null\n", render_value(v)));
                }
            }
            (None, None) => out.push_str("        # (attributes hidden)\n"),
        }
        out.push_str("    }\n");
    }
    let (add, change, destroy) = count_actions(&changes);
    out.push_str(&format!(
        "\nPlan: {add} to add, {change} to change, {destroy} to destroy.\n"
    ));
    out
}

pub fn apply_log(db: &Db, apply: &Value) -> String {
    if let Some(log) = apply.get("log").and_then(Value::as_str) {
        return log.to_owned();
    }
    let plan = opt_str(apply, "run")
        .and_then(|r| db.get("runs", r))
        .and_then(|r| opt_str(r, "plan"))
        .and_then(|p| db.get("plans", p));
    let mut out =
        String::from("Terraform v1.9.5\non linux_amd64\nInitializing plugins and modules...\n");
    for (i, c) in plan.map(changes_of).unwrap_or_default().iter().enumerate() {
        let address = str_field(c, "address");
        let actions: Vec<&str> = c["actions"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let secs = 3 + (i * 7) % 40;
        match actions.as_slice() {
            ["delete"] => out.push_str(&format!(
                "{address}: Destroying...\n{address}: Destruction complete after {secs}s\n"
            )),
            ["update"] => out.push_str(&format!(
                "{address}: Modifying...\n{address}: Modifications complete after {secs}s\n"
            )),
            _ => out.push_str(&format!(
                "{address}: Creating...\n{address}: Creation complete after {secs}s\n"
            )),
        }
    }
    let n = |k: &str| apply.get(k).and_then(Value::as_u64).unwrap_or(0);
    match str_field(apply, "status") {
        "errored" => out.push_str("\nError: apply failed\n"),
        _ => out.push_str(&format!(
            "\nApply complete! Resources: {} added, {} changed, {} destroyed.\n",
            n("resource-additions"),
            n("resource-changes"),
            n("resource-destructions")
        )),
    }
    out
}

fn provider_of(address: &str) -> String {
    let kind = address
        .split('.')
        .find(|seg| seg.contains('_'))
        .unwrap_or(address);
    let provider = kind.split('_').next().unwrap_or("null");
    format!("registry.terraform.io/hashicorp/{provider}")
}

pub fn plan_json(db: &Db, plan: &Value) -> Value {
    if let Some(json) = plan.get("json-output") {
        return json.clone();
    }
    let run = opt_str(plan, "run").and_then(|r| db.get("runs", r));
    let tf = run
        .map(|r| str_field(r, "terraform-version").to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "1.9.5".to_owned());
    let changes: Vec<Value> = changes_of(plan)
        .iter()
        .map(|c| {
            let address = str_field(c, "address");
            let (kind, name) = address
                .rsplit_once('.')
                .map(|(k, n)| (k.rsplit('.').next().unwrap_or(k).to_owned(), n.to_owned()))
                .unwrap_or((address.to_owned(), String::new()));
            json!({
                "address": address,
                "mode": "managed",
                "type": kind,
                "name": name,
                "provider_name": provider_of(address),
                "change": {
                    "actions": c.get("actions").cloned().unwrap_or(json!(["no-op"])),
                    "before": c.get("before").cloned().unwrap_or(Value::Null),
                    "after": c.get("after").cloned().unwrap_or(Value::Null),
                    "after_unknown": {},
                    "before_sensitive": {},
                    "after_sensitive": {}
                }
            })
        })
        .collect();
    let errored = str_field(plan, "status") == "errored";
    json!({
        "format_version": "1.2",
        "terraform_version": tf,
        "planned_values": { "root_module": {} },
        "resource_changes": changes,
        "configuration": { "root_module": {} },
        "timestamp": plan.pointer("/status-timestamps/finished-at").cloned().unwrap_or(Value::Null),
        "applyable": plan.get("has-changes").cloned().unwrap_or(json!(false)),
        "complete": !errored,
        "errored": errored
    })
}

// ---------------------------------------------------------------- policy checks

async fn list_policy_checks(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    if db.get("runs", &id).is_none() {
        return not_found();
    }
    let items = db
        .filter("policy_checks", |p| str_field(p, "run") == id)
        .into_iter()
        .map(resources::policy_check)
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/runs/{id}/policy-checks"),
        &ctx.q,
        items,
        vec![],
    )
}

async fn get_policy_check(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    match Db::new(&world).get("policy_checks", &id) {
        Some(p) => data(StatusCode::OK, resources::policy_check(p)),
        None => not_found(),
    }
}

// ---------------------------------------------------------------- plan exports

async fn create_plan_export(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let data_type = attributes(&doc)
        .get("data-type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if data_type != "sentinel-mock-bundle-v0" {
        return crate::common::invalid("Data type is not included in the list", "data-type");
    }
    let plan_id = relationship_id(&doc, "plan").unwrap_or_default();
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    match Db::new(world).get("plans", &plan_id) {
        None => return not_found(),
        Some(p) if str_field(p, "status") != "finished" => {
            return crate::common::invalid(
                "Plan must be finished before it can be exported",
                "plan",
            );
        }
        Some(_) => {}
    }
    let now = world.clock.now_rfc3339();
    let id = new_id(world, "plan_exports", "plan_export", "pe-");
    let export = json!({
        "id": id,
        "plan": plan_id,
        "data-type": data_type,
        "status": "queued",
        "status-timestamps": { "queued-at": now },
        "created-at": now,
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("plan_exports")
        .insert(id.clone(), export.clone());
    data(StatusCode::CREATED, resources::plan_export(&export))
}

/// Exports finish as soon as they are read again.
async fn get_plan_export(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    let Some(export) = Db::new(&world).get("plan_exports", &id).cloned() else {
        return not_found();
    };
    let mut export = export;
    if str_field(&export, "status") == "queued" {
        export["status"] = json!("finished");
        export["status-timestamps"]["finished-at"] = export["created-at"].clone();
    }
    data(StatusCode::OK, resources::plan_export(&export))
}

async fn delete_plan_export(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let mut world = world.lock().expect("world lock");
    match world
        .service_mut(NAME)
        .store
        .collection_mut("plan_exports")
        .remove(&id)
    {
        Some(_) => crate::common::no_content(),
        None => not_found(),
    }
}

async fn download_plan_export(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    if Db::new(&world).get("plan_exports", &id).is_none() {
        return not_found();
    }
    let mut resp = redirect(&format!(
        "{}/archivist/v1/object/plan-export-{id}",
        ctx.base
    ));
    *resp.status_mut() = StatusCode::FOUND;
    resp
}

// ---------------------------------------------------------------- archivist

fn sliced_log(text: &str, ctx: &Ctx) -> Response {
    let mut content = Vec::with_capacity(text.len() + 2);
    content.push(2u8);
    content.extend_from_slice(text.as_bytes());
    content.push(3u8);
    let offset = ctx
        .q
        .get("offset")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0)
        .min(content.len());
    let limit = ctx
        .q
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(content.len());
    let end = offset.saturating_add(limit).min(content.len());
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain")],
        content[offset..end].to_vec(),
    )
        .into_response()
}

fn state_file(db: &Db, sv: &Value) -> Value {
    let ws = opt_str(sv, "workspace").and_then(|w| db.get("workspaces", w));
    let outputs: serde_json::Map<String, Value> = sv
        .get("outputs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|o| {
            (
                str_field(o, "name").to_owned(),
                json!({ "value": o.get("value").cloned().unwrap_or(Value::Null), "type": o.get("type").cloned().unwrap_or(json!("string")), "sensitive": o.get("sensitive").cloned().unwrap_or(json!(false)) }),
            )
        })
        .collect();
    let resources: Vec<Value> = sv
        .get("resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| {
            let count = r.get("count").and_then(Value::as_u64).unwrap_or(1);
            json!({
                "mode": "managed",
                "type": str_field(r, "type"),
                "name": str_field(r, "name"),
                "provider": format!("provider[\"{}\"]", str_field(r, "provider")),
                "instances": (0..count).map(|i| json!({ "index_key": if count > 1 { json!(i) } else { Value::Null }, "schema_version": 0, "attributes": {} })).collect::<Vec<_>>()
            })
        })
        .collect();
    json!({
        "version": 4,
        "terraform_version": str_field(sv, "terraform-version"),
        "serial": sv.get("serial").cloned().unwrap_or(json!(1)),
        "lineage": crate::common::hex_digest(ws.map(|w| str_field(w, "id")).unwrap_or("")),
        "outputs": outputs,
        "resources": resources,
        "check_results": null
    })
}

async fn archivist_object(
    State(world): State<SharedWorld>,
    Path(key): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let json_body = |v: Value| {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            v.to_string(),
        )
            .into_response()
    };
    if let Some(id) = key.strip_prefix("plan-log-") {
        return match db.get("plans", id) {
            Some(p) => sliced_log(&plan_log(&db, p), &ctx),
            None => not_found(),
        };
    }
    if let Some(id) = key.strip_prefix("apply-log-") {
        return match db.get("applies", id) {
            Some(a) => sliced_log(&apply_log(&db, a), &ctx),
            None => not_found(),
        };
    }
    if let Some(id) = key.strip_prefix("plan-json-") {
        return match db.get("plans", id) {
            Some(p) => json_body(plan_json(&db, p)),
            None => not_found(),
        };
    }
    if let Some(id) = key
        .strip_prefix("state-json-")
        .or_else(|| key.strip_prefix("state-"))
    {
        return match db.get("state_versions", id) {
            Some(sv) => json_body(state_file(&db, sv)),
            None => not_found(),
        };
    }
    if let Some(id) = key.strip_prefix("plan-export-") {
        let Some(export) = db.get("plan_exports", id) else {
            return not_found();
        };
        let Some(plan) = db.get("plans", str_field(export, "plan")) else {
            return not_found();
        };
        let bundle = tarball::sentinel_mock_bundle(&plan_json(&db, plan), world.clock.now_unix());
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/x-gzip")],
            bundle,
        )
            .into_response();
    }
    not_found()
}
