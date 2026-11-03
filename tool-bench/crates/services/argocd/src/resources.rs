//! Application resources: resource tree, managed resources, live manifests, events,
//! resource actions and pod logs.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use mock_core::{http::json, store::Store, world::SharedWorld};
use serde_json::{Value, json};

use crate::{
    NAME, Params,
    error::{ApiResult, ArgoError},
    model::{self, app_key, app_resources, require_app, str_field, tree_node},
};

pub async fn tree(
    State(world): State<SharedWorld>,
    Path(name): Path<String>,
    params: Params,
) -> ApiResult {
    let world = world.lock().expect("world lock");
    let store = model::store(&world);
    let key = app_key(params.get("appNamespace"), &name);
    require_app(store, &key)?;
    let nodes: Vec<Value> = app_resources(store, &key)
        .into_iter()
        .map(tree_node)
        .collect();
    Ok(json(StatusCode::OK, json!({ "nodes": nodes })))
}

fn manifest_string(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "null".to_owned(),
        Some(v) => v.to_string(),
    }
}

pub async fn managed(
    State(world): State<SharedWorld>,
    Path(name): Path<String>,
    params: Params,
) -> ApiResult {
    let world = world.lock().expect("world lock");
    let store = model::store(&world);
    let key = app_key(params.get("appNamespace"), &name);
    let app = require_app(store, &key)?;
    if let Some(project) = params.non_empty("project")
        && str_field(app, "/spec/project") != project
    {
        return Err(ArgoError::app_not_found(&name));
    }
    let filter = |field: &str, r: &Value| {
        params
            .non_empty(field)
            .is_none_or(|want| str_field(r, &format!("/{field}")) == want)
    };
    let items: Vec<Value> = app_resources(store, &key)
        .into_iter()
        .filter(|r| r["managed"] == true)
        .filter(|r| {
            filter("kind", r)
                && filter("namespace", r)
                && filter("name", r)
                && filter("version", r)
                && filter("group", r)
        })
        .map(|r| {
            let live = if r["requiresPruning"] == true || r.get("manifest").is_some() {
                r.get("manifest")
            } else {
                None
            };
            let target = if r["requiresPruning"] == true {
                None
            } else {
                r.get("targetManifest")
                    .filter(|t| !t.is_null())
                    .or(r.get("manifest"))
            };
            let mut item = json!({
                "kind": str_field(r, "/kind"),
                "namespace": str_field(r, "/namespace"),
                "name": str_field(r, "/name"),
                "targetState": manifest_string(target),
                "liveState": manifest_string(live),
                "diff": "",
                "hook": false,
                "normalizedLiveState": manifest_string(live),
                "predictedLiveState": manifest_string(target),
                "resourceVersion": str_field(r, "/resourceVersion"),
                "modified": r["status"] == "OutOfSync",
            });
            let group = str_field(r, "/group");
            if !group.is_empty() {
                item["group"] = json!(group);
            }
            item
        })
        .collect();
    Ok(json(StatusCode::OK, json!({ "items": items })))
}

/// The resource named by the `namespace`/`resourceName`/`group`/`kind` query.
fn query_resource<'a>(
    store: &'a Store,
    key: &str,
    app_name: &str,
    params: &Params,
) -> Result<&'a Value, ArgoError> {
    let group = params.get("group").unwrap_or("");
    let kind = params.get("kind").unwrap_or("");
    let namespace = params.get("namespace").unwrap_or("");
    let name = params.get("resourceName").unwrap_or("");
    model::find_resource(store, key, group, kind, namespace, name).ok_or_else(|| {
        ArgoError::invalid(format!(
            "{kind} {group} {name} not found as part of application {app_name}"
        ))
    })
}

pub async fn get_resource(
    State(world): State<SharedWorld>,
    Path(name): Path<String>,
    params: Params,
) -> ApiResult {
    let world = world.lock().expect("world lock");
    let store = model::store(&world);
    let key = app_key(params.get("appNamespace"), &name);
    require_app(store, &key)?;
    let resource = query_resource(store, &key, &name, &params)?;
    Ok(json(
        StatusCode::OK,
        json!({ "manifest": manifest_string(resource.get("manifest")) }),
    ))
}

pub async fn events(
    State(world): State<SharedWorld>,
    Path(name): Path<String>,
    params: Params,
) -> ApiResult {
    let world = world.lock().expect("world lock");
    let store = model::store(&world);
    let key = app_key(params.get("appNamespace"), &name);
    let app = require_app(store, &key)?;
    let uid = params.non_empty("resourceUID");
    let res_name = params.non_empty("resourceName");
    let res_ns = params.non_empty("resourceNamespace");
    let app_level = uid.is_none() && res_name.is_none();
    let items: Vec<Value> = store
        .collection("events")
        .map(|c| {
            c.values()
                .filter(|e| {
                    if app_level {
                        str_field(e, "/involvedObject/kind") == "Application"
                            && str_field(e, "/involvedObject/name")
                                == str_field(app, "/metadata/name")
                            && str_field(e, "/involvedObject/namespace")
                                == str_field(app, "/metadata/namespace")
                    } else {
                        uid.is_none_or(|u| str_field(e, "/involvedObject/uid") == u)
                            && res_name.is_none_or(|n| str_field(e, "/involvedObject/name") == n)
                            && res_ns.is_none_or(|n| str_field(e, "/involvedObject/namespace") == n)
                    }
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Ok(json(
        StatusCode::OK,
        json!({ "metadata": {}, "items": items }),
    ))
}

/// Built-in resource actions (Argo CD's Lua actions) and whether each is disabled.
fn actions_for(resource: &Value) -> Vec<(&'static str, &'static str, bool)> {
    let group = str_field(resource, "/group");
    let kind = str_field(resource, "/kind");
    let manifest = resource.get("manifest").unwrap_or(&Value::Null);
    let paused = manifest.pointer("/spec/paused") == Some(&json!(true));
    let suspended = manifest.pointer("/spec/suspend") == Some(&json!(true));
    match (group, kind) {
        ("apps", "Deployment") => vec![
            ("restart", "fa fa-fw fa-redo", false),
            ("pause", "fa fa-fw fa-pause-circle", paused),
            ("resume", "fa fa-fw fa-play-circle", !paused),
        ],
        ("apps", "StatefulSet") | ("apps", "DaemonSet") => {
            vec![("restart", "fa fa-fw fa-redo", false)]
        }
        ("batch", "CronJob") => vec![
            ("suspend", "fa fa-fw fa-pause", suspended),
            ("resume", "fa fa-fw fa-play", !suspended),
        ],
        ("argoproj.io", "Rollout") => vec![
            ("restart", "fa fa-fw fa-redo", false),
            ("pause", "fa fa-fw fa-pause-circle", paused),
            ("resume", "fa fa-fw fa-play-circle", !paused),
            ("promote-full", "fa fa-fw fa-angle-double-right", false),
            ("abort", "fa fa-fw fa-times-circle", false),
            ("retry", "fa fa-fw fa-redo-alt", false),
        ],
        _ => Vec::new(),
    }
}

fn display_name(action: &str) -> String {
    action
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("-")
}

pub async fn list_actions(
    State(world): State<SharedWorld>,
    Path(name): Path<String>,
    params: Params,
) -> ApiResult {
    let world = world.lock().expect("world lock");
    let store = model::store(&world);
    let key = app_key(params.get("appNamespace"), &name);
    require_app(store, &key)?;
    let resource = query_resource(store, &key, &name, &params)?;
    let actions: Vec<Value> = actions_for(resource)
        .into_iter()
        .map(|(action, icon, disabled)| {
            json!({
                "name": action,
                "disabled": disabled,
                "iconClass": icon,
                "displayName": display_name(action),
            })
        })
        .collect();
    Ok(json(StatusCode::OK, json!({ "actions": actions })))
}

/// Applies an action's Lua script effect to the live manifest.
fn apply_action(manifest: &mut Value, kind: &str, action: &str, now: &str) {
    let set = |m: &mut Value, pointer: &[&str], v: Value| {
        let mut cur = m;
        for seg in &pointer[..pointer.len() - 1] {
            if !cur[*seg].is_object() {
                cur[*seg] = json!({});
            }
            cur = &mut cur[*seg];
        }
        cur[pointer[pointer.len() - 1]] = v;
    };
    match (kind, action) {
        ("Rollout", "restart") => set(manifest, &["spec", "restartAt"], json!(now)),
        (_, "restart") => set(
            manifest,
            &[
                "spec",
                "template",
                "metadata",
                "annotations",
                "kubectl.kubernetes.io/restartedAt",
            ],
            json!(now),
        ),
        (_, "pause") => set(manifest, &["spec", "paused"], json!(true)),
        ("CronJob", "suspend") => set(manifest, &["spec", "suspend"], json!(true)),
        ("CronJob", "resume") => set(manifest, &["spec", "suspend"], json!(false)),
        (_, "resume") => set(manifest, &["spec", "paused"], json!(false)),
        (_, "abort") => set(manifest, &["status", "abort"], json!(true)),
        (_, "retry") => set(manifest, &["status", "abort"], json!(false)),
        (_, "promote-full") => set(manifest, &["status", "promoteFull"], json!(true)),
        _ => {}
    }
}

pub async fn run_action(
    State(world): State<SharedWorld>,
    Path(name): Path<String>,
    params: Params,
    body: Bytes,
) -> ApiResult {
    // The HTTP body is the `action` field itself: a JSON string.
    let action = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::String(s)) => s,
        Ok(Value::Object(o)) => o
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        Ok(_) | Err(_) if body.is_empty() => String::new(),
        _ => return Err(ArgoError::bad_json()),
    };
    let key = app_key(params.get("appNamespace"), &name);
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_rfc3339();
    let svc = world.service_mut(NAME);
    let app = require_app(&svc.store, &key)?.clone();
    let resource = query_resource(&svc.store, &key, &name, &params)?.clone();
    let available = actions_for(&resource);
    let Some((_, _, disabled)) = available.iter().find(|(a, _, _)| *a == action) else {
        return Err(ArgoError::invalid(format!(
            "error getting Lua resource action: Unable to find action named '{action}' for resource {}/{}/{}",
            str_field(&resource, "/group"),
            str_field(&resource, "/kind"),
            str_field(&resource, "/name"),
        )));
    };
    if *disabled {
        return Err(ArgoError::invalid(format!("action {action} is disabled")));
    }
    let uid = str_field(&resource, "/uid").to_owned();
    let rv = model::next_resource_version(svc);
    let kind = str_field(&resource, "/kind").to_owned();
    if let Some(r) = svc.store.collection_mut("resources").get_mut(&uid) {
        apply_action(&mut r["manifest"], &kind, &action, &now);
        r["resourceVersion"] = json!(rv);
        r["manifest"]["metadata"]["resourceVersion"] = json!(rv);
    }
    let message = format!(
        "admin ran action {action} on resource {}/{}/{}",
        str_field(&resource, "/group"),
        kind,
        str_field(&resource, "/name"),
    );
    model::app_event(
        svc,
        &now,
        &app,
        "argocd-server",
        "ResourceActionRan",
        &message,
    );
    Ok(json(StatusCode::OK, json!({})))
}

fn stream(body: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

/// Newline-delimited `{"result": LogEntry}` messages, ending with a `last` entry.
fn log_stream(store: &Store, pods: &[&Value], params: &Params) -> String {
    let container = params.non_empty("container");
    let tail = params
        .get("tailLines")
        .and_then(|t| t.parse::<usize>().ok())
        .filter(|t| *t > 0);
    let filter = params.non_empty("filter");
    let mut out = String::new();
    for pod in pods {
        let pod_name = str_field(pod, "/name");
        let pod_ns = str_field(pod, "/namespace");
        // Without a container, the pod's first container (as kubectl does).
        let first_container = pod
            .pointer("/manifest/spec/containers/0/name")
            .and_then(Value::as_str);
        let wanted = container.or(first_container);
        for log in store
            .collection("logs")
            .into_iter()
            .flat_map(|c| c.values())
        {
            if str_field(log, "/podName") != pod_name
                || str_field(log, "/namespace") != pod_ns
                || wanted.is_some_and(|c| str_field(log, "/container") != c)
            {
                continue;
            }
            let lines: Vec<&Value> = log["lines"]
                .as_array()
                .map(|l| {
                    l.iter()
                        .filter(|line| {
                            filter.is_none_or(|f| str_field(line, "/content").contains(f))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let skip = tail.map_or(0, |t| lines.len().saturating_sub(t));
            for line in &lines[skip..] {
                let ts = str_field(line, "/timeStamp");
                out.push_str(
                    &json!({
                        "result": {
                            "content": str_field(line, "/content"),
                            "timeStamp": ts,
                            "last": false,
                            "timeStampStr": ts,
                            "podName": pod_name,
                        }
                    })
                    .to_string(),
                );
                out.push('\n');
            }
        }
    }
    out.push_str(
        &json!({ "result": { "content": "", "last": true, "timeStampStr": "", "podName": "" } })
            .to_string(),
    );
    out.push('\n');
    out
}

fn pods_under<'a>(store: &'a Store, key: &str, root_uid: Option<&str>) -> Vec<&'a Value> {
    let scope: Option<Vec<String>> = root_uid.map(|u| model::descendants(store, u));
    app_resources(store, key)
        .into_iter()
        .filter(|r| str_field(r, "/kind") == "Pod" && str_field(r, "/group").is_empty())
        .filter(|r| {
            scope
                .as_ref()
                .is_none_or(|s| s.iter().any(|u| u == str_field(r, "/uid")))
        })
        .collect()
}

pub async fn logs(
    State(world): State<SharedWorld>,
    Path(name): Path<String>,
    params: Params,
) -> Response {
    let world = world.lock().expect("world lock");
    let store = model::store(&world);
    let key = app_key(params.get("appNamespace"), &name);
    if let Err(e) = require_app(store, &key) {
        return stream(e.stream_chunk());
    }
    let pods = if let Some(pod) = params.non_empty("podName") {
        let ns = params.get("namespace").unwrap_or("");
        match model::find_resource(store, &key, "", "Pod", ns, pod) {
            Some(p) => vec![p],
            None => {
                return stream(
                    ArgoError::invalid(format!(
                        "Pod  {pod} not found as part of application {name}"
                    ))
                    .stream_chunk(),
                );
            }
        }
    } else if params.non_empty("resourceName").is_some() {
        match query_resource(store, &key, &name, &params) {
            Ok(r) => pods_under(store, &key, Some(str_field(r, "/uid"))),
            Err(e) => return stream(e.stream_chunk()),
        }
    } else {
        pods_under(store, &key, None)
    };
    stream(log_stream(store, &pods, &params))
}

pub async fn pod_logs(
    State(world): State<SharedWorld>,
    Path((name, pod)): Path<(String, String)>,
    params: Params,
) -> Response {
    let world = world.lock().expect("world lock");
    let store = model::store(&world);
    let key = app_key(params.get("appNamespace"), &name);
    if let Err(e) = require_app(store, &key) {
        return stream(e.stream_chunk());
    }
    let found: Vec<&Value> = app_resources(store, &key)
        .into_iter()
        .filter(|r| str_field(r, "/kind") == "Pod" && str_field(r, "/name") == pod)
        .filter(|r| {
            params
                .non_empty("namespace")
                .is_none_or(|ns| str_field(r, "/namespace") == ns)
        })
        .take(1)
        .collect();
    if found.is_empty() {
        return stream(
            ArgoError::invalid(format!(
                "Pod  {pod} not found as part of application {name}"
            ))
            .stream_chunk(),
        );
    }
    stream(log_stream(store, &found, &params))
}
