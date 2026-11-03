//! The Argo CD world model over the store.
//!
//! Collections (all grader-addressable):
//! - `applications`: `<app namespace>/<name>` -> v1alpha1 Application. `status.resources` and
//!   `status.summary` are derived from `resources` when served.
//! - `projects`: `<name>` -> AppProject.
//! - `clusters`: `<name>` -> Cluster.
//! - `resources`: `<uid>` -> a live Kubernetes object in an application's tree: the
//!   ResourceNode fields (`group`, `version`, `kind`, `namespace`, `name`, `uid`, `parentRefs`,
//!   `info`, `networkingInfo`, `images`, `health`, `createdAt`, `resourceVersion`) plus
//!   `app` (application key), `managed` (top-level, in git), `status` (Synced/OutOfSync),
//!   `requiresPruning`, `manifest` (live object) and optional `targetManifest`.
//! - `events`: `<uid>` -> core/v1 Event (applications and their resources).
//! - `logs`: `<pod>/<container>` -> `{podName, namespace, container, lines: [{timeStamp, content}]}`.
//! - `revisions`: `<sha>` -> `{repoURL, sha, refs, message, author, date, health?}`, the git
//!   commits the repo server can resolve. `health` is the application health once synced.

use std::sync::LazyLock;

use mock_core::{
    store::Store,
    world::{ServiceState, WorldState},
};
use serde_json::{Map, Value, json};

use crate::{NAME, error::ArgoError};

pub const DEFAULT_APP_NAMESPACE: &str = "argocd";
const DEFAULT_HISTORY_LIMIT: usize = 10;

static EMPTY: LazyLock<Store> = LazyLock::new(Store::default);

pub fn store(world: &WorldState) -> &Store {
    world.service(NAME).map_or(&EMPTY, |s| &s.store)
}

pub fn app_key(namespace: Option<&str>, name: &str) -> String {
    let ns = namespace
        .filter(|n| !n.is_empty())
        .unwrap_or(DEFAULT_APP_NAMESPACE);
    format!("{ns}/{name}")
}

pub fn get_app<'a>(store: &'a Store, key: &str) -> Option<&'a Value> {
    store.collection("applications").and_then(|c| c.get(key))
}

pub fn require_app<'a>(store: &'a Store, key: &str) -> Result<&'a Value, ArgoError> {
    get_app(store, key).ok_or_else(|| ArgoError::app_not_found(key_name(key)))
}

fn key_name(key: &str) -> &str {
    key.split_once('/').map_or(key, |(_, n)| n)
}

pub fn str_field<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

/// Live objects of one application, in dataset order.
pub fn app_resources<'a>(store: &'a Store, key: &str) -> Vec<&'a Value> {
    store
        .collection("resources")
        .map(|c| c.values().filter(|r| r["app"] == key).collect())
        .unwrap_or_default()
}

/// A resource of the application matching group/kind/namespace/name (version is not compared).
pub fn find_resource<'a>(
    store: &'a Store,
    key: &str,
    group: &str,
    kind: &str,
    namespace: &str,
    name: &str,
) -> Option<&'a Value> {
    app_resources(store, key).into_iter().find(|r| {
        str_field(r, "/group") == group
            && str_field(r, "/kind") == kind
            && str_field(r, "/namespace") == namespace
            && str_field(r, "/name") == name
    })
}

/// `<uid>` plus the uids of every resource below it in the tree.
pub fn descendants(store: &Store, root_uid: &str) -> Vec<String> {
    let all: Vec<&Value> = store
        .collection("resources")
        .map(|c| c.values().collect())
        .unwrap_or_default();
    let mut out = vec![root_uid.to_owned()];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i].clone();
        for r in &all {
            let is_child = r["parentRefs"]
                .as_array()
                .is_some_and(|refs| refs.iter().any(|p| p["uid"] == parent.as_str()));
            let uid = str_field(r, "/uid");
            if is_child && !out.iter().any(|u| u == uid) {
                out.push(uid.to_owned());
            }
        }
        i += 1;
    }
    out
}

/// A ResourceNode as served in the resource tree.
pub fn tree_node(r: &Value) -> Value {
    let mut node = Map::new();
    for field in [
        "version",
        "kind",
        "namespace",
        "name",
        "uid",
        "group",
        "parentRefs",
        "info",
        "networkingInfo",
        "resourceVersion",
        "images",
        "health",
        "createdAt",
    ] {
        match r.get(field) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.is_empty() => {}
            Some(v) => {
                node.insert(field.to_owned(), v.clone());
            }
        }
    }
    Value::Object(node)
}

/// A ResourceStatus entry of `status.resources`.
fn resource_status(r: &Value) -> Value {
    let mut out = Map::new();
    for field in ["group", "version", "kind", "namespace", "name", "status"] {
        if let Some(Value::String(s)) = r.get(field)
            && !s.is_empty()
        {
            out.insert(field.to_owned(), json!(s));
        }
    }
    if let Some(health) = r.get("health").filter(|h| !h.is_null()) {
        out.insert("health".to_owned(), health.clone());
    }
    if r["requiresPruning"] == true {
        out.insert("requiresPruning".to_owned(), json!(true));
    }
    Value::Object(out)
}

/// The Application as the API serves it, with derived `status.resources` and `status.summary`.
pub fn render_app(store: &Store, key: &str) -> Option<Value> {
    let mut app = get_app(store, key)?.clone();
    let resources = app_resources(store, key);
    let managed: Vec<Value> = resources
        .iter()
        .filter(|r| r["managed"] == true)
        .map(|r| resource_status(r))
        .collect();
    let mut images: Vec<String> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for r in &resources {
        for image in r["images"].as_array().into_iter().flatten() {
            if let Some(s) = image.as_str()
                && !images.iter().any(|i| i == s)
            {
                images.push(s.to_owned());
            }
        }
        for url in r
            .pointer("/networkingInfo/externalURLs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(s) = url.as_str()
                && !urls.iter().any(|u| u == s)
            {
                urls.push(s.to_owned());
            }
        }
    }
    let mut summary = Map::new();
    if !urls.is_empty() {
        summary.insert("externalURLs".to_owned(), json!(urls));
    }
    if !images.is_empty() {
        summary.insert("images".to_owned(), json!(images));
    }
    let status = app
        .as_object_mut()?
        .entry("status")
        .or_insert_with(|| json!({}));
    if let Some(status) = status.as_object_mut() {
        if !managed.is_empty() {
            status.insert("resources".to_owned(), Value::Array(managed));
        }
        status.insert("summary".to_owned(), Value::Object(summary));
    }
    Some(app)
}

fn normalize_repo(url: &str) -> &str {
    let url = url.trim_end_matches('/');
    url.strip_suffix(".git").unwrap_or(url)
}

/// The commit a revision (branch, tag, `HEAD`, full or short SHA) resolves to in `repo_url`.
pub fn resolve_revision<'a>(store: &'a Store, repo_url: &str, revision: &str) -> Option<&'a Value> {
    let commits: Vec<&Value> = store
        .collection("revisions")
        .map(|c| {
            c.values()
                .filter(|r| normalize_repo(str_field(r, "/repoURL")) == normalize_repo(repo_url))
                .collect()
        })
        .unwrap_or_default();
    let has_ref = |c: &&&Value, name: &str| {
        c["refs"]
            .as_array()
            .is_some_and(|refs| refs.iter().any(|r| r == name))
    };
    let rev = revision.trim();
    if rev.is_empty() || rev == "HEAD" {
        for name in ["HEAD", "main", "master"] {
            if let Some(c) = commits.iter().find(|c| has_ref(c, name)) {
                return Some(c);
            }
        }
        return None;
    }
    if let Some(c) = commits.iter().find(|c| has_ref(c, rev)) {
        return Some(c);
    }
    let rev = rev.to_ascii_lowercase();
    if rev.len() >= 7 && rev.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return commits
            .iter()
            .find(|c| str_field(c, "/sha").starts_with(&rev))
            .copied();
    }
    None
}

pub fn repo_known(store: &Store, repo_url: &str) -> bool {
    store.collection("revisions").is_some_and(|c| {
        c.values()
            .any(|r| normalize_repo(str_field(r, "/repoURL")) == normalize_repo(repo_url))
    })
}

/// `*`-only glob, as Argo CD uses for project source repos and destinations.
pub fn glob(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some((b'*', rest)) => (0..=t.len()).any(|i| go(rest, &t[i..])),
            Some((c, rest)) => t.first() == Some(c) && go(rest, &t[1..]),
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

pub fn cluster_by<'a>(store: &'a Store, field: &str, value: &str) -> Option<&'a Value> {
    store
        .collection("clusters")
        .and_then(|c| c.values().find(|cl| cl[field] == value))
}

/// Validates an application spec the way the API server does before create/update.
pub fn validate_spec(
    store: &Store,
    name: &str,
    app_ns: &str,
    spec: &Value,
) -> Result<(), ArgoError> {
    let invalid = |msg: String| {
        ArgoError::invalid(format!(
            "application spec for {name} is invalid: InvalidSpecError: {msg}"
        ))
    };
    let project_name = str_field(spec, "/project");
    let Some(project) = store
        .collection("projects")
        .and_then(|c| c.get(project_name))
    else {
        return Err(invalid(format!(
            "application references project {project_name} which does not exist"
        )));
    };
    if app_ns != DEFAULT_APP_NAMESPACE {
        let allowed = project
            .pointer("/spec/sourceNamespaces")
            .and_then(Value::as_array)
            .is_some_and(|ns| {
                ns.iter()
                    .any(|n| n.as_str().is_some_and(|n| glob(n, app_ns)))
            });
        if !allowed {
            return Err(ArgoError::permission_denied(format!(
                "app is not allowed in project \"{project_name}\", or the project does not exist"
            )));
        }
    }

    let dest_server = str_field(spec, "/destination/server");
    let dest_name = str_field(spec, "/destination/name");
    let dest_ns = str_field(spec, "/destination/namespace");
    if !dest_server.is_empty() && !dest_name.is_empty() {
        return Err(invalid(format!(
            "application destination can't have both name and server defined: {dest_name} {dest_server}"
        )));
    }
    let cluster = if !dest_name.is_empty() {
        cluster_by(store, "name", dest_name).ok_or_else(|| {
            invalid(format!(
                "Unable to find destination server: there are no clusters with this name: {dest_name}"
            ))
        })?
    } else if !dest_server.is_empty() {
        cluster_by(store, "server", dest_server).ok_or_else(|| {
            invalid(format!(
                "Unable to find destination server: there are no clusters with this URL: {dest_server}"
            ))
        })?
    } else {
        return Err(invalid(
            "Destination server missing from app spec".to_owned(),
        ));
    };
    let server = str_field(cluster, "/server");
    let cluster_name = str_field(cluster, "/name");

    let repo = str_field(spec, "/source/repoURL");
    let repo_ok = project
        .pointer("/spec/sourceRepos")
        .and_then(Value::as_array)
        .is_some_and(|repos| {
            repos
                .iter()
                .any(|r| r.as_str().is_some_and(|r| glob(r, repo)))
        });
    if !repo_ok {
        return Err(invalid(format!(
            "application repo {repo} is not permitted in project '{project_name}'"
        )));
    }
    let dest_ok = project
        .pointer("/spec/destinations")
        .and_then(Value::as_array)
        .is_some_and(|dests| {
            dests.iter().any(|d| {
                let ns_ok = glob(str_field(d, "/namespace"), dest_ns);
                let server_ok = d["server"].as_str().is_some_and(|p| glob(p, server))
                    || d["name"].as_str().is_some_and(|p| glob(p, cluster_name));
                ns_ok && server_ok
            })
        });
    if !dest_ok {
        return Err(invalid(format!(
            "application destination server '{server}' and namespace '{dest_ns}' do not match any of the allowed destinations in project '{project_name}'"
        )));
    }

    if !repo_known(store, repo) {
        return Err(invalid(
            "repository not accessible: repository not found".to_owned(),
        ));
    }
    let target = str_field(spec, "/source/targetRevision");
    if resolve_revision(store, repo, target).is_none() {
        return Err(invalid(format!(
            "Unable to generate manifests in {}: rpc error: code = Unknown desc = unable to resolve '{}' to a commit SHA",
            str_field(spec, "/source/path"),
            if target.is_empty() { "HEAD" } else { target }
        )));
    }
    Ok(())
}

pub fn next_resource_version(svc: &mut ServiceState) -> String {
    svc.ids.next_int("resourceVersion", 2_000_000).to_string()
}

/// Records an event on the application (as the API server or controller would).
pub fn app_event(
    svc: &mut ServiceState,
    now: &str,
    app: &Value,
    component: &str,
    reason: &str,
    message: &str,
) {
    let uid = svc.ids.next_uuid("event").to_string();
    let suffix: String = uid.chars().filter(|c| *c != '-').take(16).collect();
    let rv = next_resource_version(svc);
    let name = str_field(app, "/metadata/name");
    let ns = str_field(app, "/metadata/namespace");
    let event = json!({
        "metadata": {
            "name": format!("{name}.{suffix}"),
            "namespace": ns,
            "uid": uid,
            "resourceVersion": rv,
            "creationTimestamp": now,
        },
        "involvedObject": {
            "kind": "Application",
            "namespace": ns,
            "name": name,
            "uid": app.pointer("/metadata/uid").cloned().unwrap_or(Value::Null),
            "apiVersion": "argoproj.io/v1alpha1",
            "resourceVersion": app.pointer("/metadata/resourceVersion").cloned().unwrap_or(Value::Null),
        },
        "reason": reason,
        "message": message,
        "source": { "component": component },
        "firstTimestamp": now,
        "lastTimestamp": now,
        "count": 1,
        "type": "Normal",
        "eventTime": null,
        "reportingComponent": "",
        "reportingInstance": "",
    });
    svc.store.collection_mut("events").insert(uid, event);
}

#[derive(Debug, Clone, Default)]
pub struct SyncRequest {
    pub revision: Option<String>,
    pub prune: bool,
    pub dry_run: bool,
    pub sync_options: Option<Value>,
    /// Started by the controller's automated sync rather than a user.
    pub automated: bool,
}

fn kubectl_name(r: &Value) -> String {
    let kind = str_field(r, "/kind").to_ascii_lowercase();
    let group = str_field(r, "/group");
    let name = str_field(r, "/name");
    if group.is_empty() {
        format!("{kind}/{name}")
    } else {
        format!("{kind}.{group}/{name}")
    }
}

fn deployed_revision(app: &Value) -> Option<&str> {
    app.pointer("/status/history")
        .and_then(Value::as_array)
        .and_then(|h| h.last())
        .and_then(|h| h["revision"].as_str())
}

/// Sync status of `key` from its target revision and managed resources.
fn sync_status(store: &Store, key: &str, app: &Value, deployed: Option<&str>) -> (String, Value) {
    let repo = str_field(app, "/spec/source/repoURL");
    let target = str_field(app, "/spec/source/targetRevision");
    let target_sha = resolve_revision(store, repo, target).map(|c| str_field(c, "/sha").to_owned());
    let drifted = app_resources(store, key)
        .iter()
        .any(|r| r["managed"] == true && r["status"] == "OutOfSync");
    let status = if !drifted && target_sha.is_some() && target_sha.as_deref() == deployed {
        "Synced"
    } else {
        "OutOfSync"
    };
    (
        status.to_owned(),
        target_sha.map_or(Value::Null, Value::String),
    )
}

fn set_sync(app: &mut Value, status: &str, revision: Value) {
    let source = app.pointer("/spec/source").cloned().unwrap_or(Value::Null);
    let destination = app
        .pointer("/spec/destination")
        .cloned()
        .unwrap_or(Value::Null);
    let mut sync = json!({
        "status": status,
        "comparedTo": { "source": source, "destination": destination },
    });
    if !revision.is_null() {
        sync["revision"] = revision;
    }
    app["status"]["sync"] = sync;
}

/// After a spec change: recompare against the new target and auto-sync if enabled.
pub fn reconcile(svc: &mut ServiceState, now: &str, key: &str) -> Result<(), ArgoError> {
    let Some(mut app) = get_app(&svc.store, key).cloned() else {
        return Ok(());
    };
    let before = str_field(&app, "/status/sync/status").to_owned();
    let (status, revision) = sync_status(&svc.store, key, &app, deployed_revision(&app));
    set_sync(&mut app, &status, revision);
    app["status"]["reconciledAt"] = json!(now);
    if before != status && !before.is_empty() {
        app_event(
            svc,
            now,
            &app,
            "argocd-application-controller",
            "ResourceUpdated",
            &format!("Updated sync status: {before} -> {status}"),
        );
    }
    let automated = app
        .pointer("/spec/syncPolicy/automated")
        .is_some_and(|a| a.is_object());
    let prune = app
        .pointer("/spec/syncPolicy/automated/prune")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    svc.store
        .collection_mut("applications")
        .insert(key.to_owned(), app);
    if automated && status == "OutOfSync" {
        sync(
            svc,
            now,
            key,
            SyncRequest {
                prune,
                automated: true,
                ..SyncRequest::default()
            },
        )?;
    }
    Ok(())
}

/// Runs a sync operation to completion (the mock's controller finishes it immediately).
pub fn sync(
    svc: &mut ServiceState,
    now: &str,
    key: &str,
    req: SyncRequest,
) -> Result<(), ArgoError> {
    let mut app = require_app(&svc.store, key)?.clone();
    let phase = str_field(&app, "/status/operationState/phase");
    if phase == "Running" || phase == "Terminating" {
        return Err(ArgoError::failed_precondition(
            "another operation is already in progress",
        ));
    }
    let target = str_field(&app, "/spec/source/targetRevision").to_owned();
    let automated_policy = app
        .pointer("/spec/syncPolicy/automated")
        .is_some_and(|a| a.is_object());
    if let Some(rev) = req.revision.as_deref()
        && !req.automated
        && automated_policy
        && !req.dry_run
        && rev != if target.is_empty() { "HEAD" } else { &target }
    {
        return Err(ArgoError::failed_precondition(format!(
            "Cannot sync to {rev}: auto-sync currently set to {target}"
        )));
    }
    let repo = str_field(&app, "/spec/source/repoURL").to_owned();
    let wanted = req.revision.clone().unwrap_or_else(|| target.clone());
    let Some(commit) = resolve_revision(&svc.store, &repo, &wanted).cloned() else {
        let shown = if wanted.is_empty() { "HEAD" } else { &wanted };
        return Err(ArgoError::failed_precondition(format!(
            "rpc error: code = Unknown desc = unable to resolve '{shown}' to a commit SHA"
        )));
    };
    let sha = str_field(&commit, "/sha").to_owned();
    let source = app.pointer("/spec/source").cloned().unwrap_or(Value::Null);
    let initiated_by = if req.automated {
        json!({ "automated": true })
    } else {
        json!({ "username": "admin" })
    };

    let managed: Vec<Value> = app_resources(&svc.store, key)
        .into_iter()
        .filter(|r| r["managed"] == true)
        .cloned()
        .collect();
    let dry = if req.dry_run { " (dry run)" } else { "" };
    let mut results = Vec::new();
    let mut prune_uids = Vec::new();
    for r in &managed {
        let (status, message) = if r["requiresPruning"] == true {
            if req.prune {
                prune_uids.push(str_field(r, "/uid").to_owned());
                ("Pruned", format!("pruned{dry}"))
            } else {
                ("PruneSkipped", "ignored (requires pruning)".to_owned())
            }
        } else if r["status"] == "Synced" {
            ("Synced", format!("{} unchanged{dry}", kubectl_name(r)))
        } else {
            ("Synced", format!("{} configured{dry}", kubectl_name(r)))
        };
        let mut result = json!({
            "group": str_field(r, "/group"),
            "version": str_field(r, "/version"),
            "kind": str_field(r, "/kind"),
            "namespace": str_field(r, "/namespace"),
            "name": str_field(r, "/name"),
            "status": status,
            "message": message,
            "hookPhase": "Running",
            "syncPhase": "Sync",
        });
        if req.dry_run {
            result["hookPhase"] = json!("Succeeded");
        }
        results.push(result);
    }

    let mut operation_sync = json!({ "revision": sha });
    if req.prune {
        operation_sync["prune"] = json!(true);
    }
    if req.dry_run {
        operation_sync["dryRun"] = json!(true);
    }
    if let Some(options) = &req.sync_options {
        operation_sync["syncOptions"] = options.clone();
    }
    let mut operation = json!({ "sync": operation_sync, "initiatedBy": initiated_by, "retry": {} });
    if req.automated {
        operation["sync"]["prune"] = json!(req.prune);
    }

    let display = if req.automated {
        format!("initiated automated sync to '{sha}'")
    } else {
        format!("admin initiated sync to {wanted} ({sha})")
    };
    app_event(
        svc,
        now,
        &app,
        "argocd-server",
        "OperationStarted",
        &display,
    );

    let before_sync = str_field(&app, "/status/sync/status").to_owned();
    let before_health = str_field(&app, "/status/health/status").to_owned();
    if !req.dry_run {
        // Apply: prune, mark everything else synced, adopt the commit's health.
        let mut removed = Vec::new();
        for uid in &prune_uids {
            removed.extend(descendants(&svc.store, uid));
        }
        let healthy = commit
            .pointer("/health/status")
            .is_some_and(|h| h == "Healthy");
        let resources = svc.store.collection_mut("resources");
        for uid in &removed {
            resources.remove(uid);
        }
        let uids: Vec<String> = resources
            .iter()
            .filter(|(_, r)| r["app"] == key)
            .map(|(k, _)| k.to_owned())
            .collect();
        for uid in uids {
            if let Some(r) = resources.get_mut(&uid) {
                if r["managed"] == true && r["requiresPruning"] != true {
                    r["status"] = json!("Synced");
                    if let Some(target) = r.get("targetManifest").cloned()
                        && !target.is_null()
                    {
                        r["manifest"] = target;
                        if let Some(obj) = r.as_object_mut() {
                            obj.remove("targetManifest");
                        }
                    }
                }
                if healthy && r.get("health").is_some_and(|h| !h.is_null()) {
                    r["health"] = json!({ "status": "Healthy" });
                }
            }
        }
        if let Some(health) = commit.get("health").filter(|h| !h.is_null()) {
            app["status"]["health"] = health.clone();
        } else if managed.is_empty() && str_field(&app, "/status/health/status") == "Missing" {
            app["status"]["health"] = json!({ "status": "Healthy" });
        }

        let limit = app
            .pointer("/spec/revisionHistoryLimit")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_HISTORY_LIMIT, |l| l as usize);
        if !app["status"]["history"].is_array() {
            app["status"]["history"] = json!([]);
        }
        if let Some(history) = app["status"]["history"].as_array_mut() {
            let id = history
                .iter()
                .filter_map(|h| h["id"].as_i64())
                .max()
                .unwrap_or(-1)
                + 1;
            history.push(json!({
                "revision": sha,
                "deployedAt": now,
                "id": id,
                "source": source,
                "deployStartedAt": now,
                "initiatedBy": initiated_by,
            }));
            if history.len() > limit {
                let excess = history.len() - limit;
                history.drain(..excess);
            }
        }
        let (status, revision) = sync_status(&svc.store, key, &app, Some(&sha));
        set_sync(&mut app, &status, revision);
    }

    app["status"]["operationState"] = json!({
        "operation": operation,
        "phase": "Succeeded",
        "message": "successfully synced (all tasks run)",
        "syncResult": { "resources": results, "revision": sha, "source": source },
        "startedAt": now,
        "finishedAt": now,
    });
    app["status"]["reconciledAt"] = json!(now);
    if let Some(obj) = app.as_object_mut() {
        obj.remove("operation");
    }

    app_event(
        svc,
        now,
        &app,
        "argocd-application-controller",
        "OperationCompleted",
        &format!("Sync operation to {sha} succeeded"),
    );
    let after_sync = str_field(&app, "/status/sync/status").to_owned();
    if after_sync != before_sync {
        app_event(
            svc,
            now,
            &app,
            "argocd-application-controller",
            "ResourceUpdated",
            &format!("Updated sync status: {before_sync} -> {after_sync}"),
        );
    }
    let after_health = str_field(&app, "/status/health/status").to_owned();
    if after_health != before_health {
        app_event(
            svc,
            now,
            &app,
            "argocd-application-controller",
            "ResourceUpdated",
            &format!("Updated health status: {before_health} -> {after_health}"),
        );
    }
    svc.store
        .collection_mut("applications")
        .insert(key.to_owned(), app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_like_argocd() {
        assert!(glob("*", "anything"));
        assert!(glob(
            "https://github.com/acme/*",
            "https://github.com/acme/x"
        ));
        assert!(!glob("payments-*", "billing"));
        assert!(glob("payments-*", "payments-prod"));
        assert!(glob("", ""));
        assert!(!glob("", "x"));
    }
}
