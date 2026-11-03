use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mock_argocd::Argocd;
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use serde_json::{Value, json};
use tower::ServiceExt;

const DEGRADED: &str = "find-degraded-app";
const ROLLBACK: &str = "roll-back-billing-worker";
const RESTART: &str = "restart-indexer-worker";
const PAYMENTS: &str = "sync-prod-payments-api";

const WORKER_UID: &str = "d39d8921-d825-549b-a290-0dcda1cafa6c";
const LEGACY_PROD_UID: &str = "41db962d-0f81-5de8-8dc3-ced47c1d11fc";
const LEGACY_STAGING_UID: &str = "675645aa-e28d-5292-a17c-94c47c5d9f17";

fn world(slug: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/code/argocd")
        .join(slug);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn app(w: &SharedWorld) -> Router {
    service_app(w.clone(), &Argocd)
}

async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    token: Option<&str>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8108")
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn call(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
    let (status, text) = raw(app, method, uri, body, Some("argocd-toolbench")).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["argocd"].clone()
}

fn names(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            format!(
                "{}/{}",
                a["metadata"]["namespace"].as_str().unwrap(),
                a["metadata"]["name"].as_str().unwrap()
            )
        })
        .collect()
}

#[tokio::test]
async fn requires_a_bearer_token() {
    let a = app(&world(DEGRADED));
    let (status, body) = raw(&a, "GET", "/api/v1/applications", "", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({"error": "no session information", "code": 16, "message": "no session information"})
    );
    let (status, _) = raw(&a, "GET", "/api/v1/applications", "", Some("nope")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn lists_applications_sorted_with_filters() {
    let a = app(&world(DEGRADED));
    let (status, all) = call(&a, "GET", "/api/v1/applications", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        names(&all),
        [
            "argocd/catalog-api",
            "argocd/checkout-web",
            "argocd/checkout-web-staging",
            "argocd/ingress-nginx",
            "argocd/order-events-consumer"
        ]
    );
    let (_, found) = call(&a, "GET", "/api/v1/applications?search=checkout", "").await;
    assert_eq!(
        names(&found),
        ["argocd/checkout-web", "argocd/checkout-web-staging"]
    );
    let (_, by_project) = call(&a, "GET", "/api/v1/applications?projects=platform", "").await;
    assert_eq!(names(&by_project), ["argocd/ingress-nginx"]);
    let (_, by_label) = call(
        &a,
        "GET",
        "/api/v1/applications?selector=env%3Dproduction,tier%3Dfrontend",
        "",
    )
    .await;
    assert_eq!(names(&by_label), ["argocd/checkout-web"]);
    let (_, none) = call(
        &a,
        "GET",
        "/api/v1/applications?search=nothing-like-this",
        "",
    )
    .await;
    assert_eq!(none, json!({"metadata": {}, "items": []}));
}

#[tokio::test]
async fn gets_application_with_derived_status() {
    let a = app(&world(DEGRADED));
    let (status, app_doc) = call(&a, "GET", "/api/v1/applications/checkout-web", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app_doc["status"]["health"]["status"], "Degraded");
    assert_eq!(app_doc["status"]["sync"]["status"], "Synced");
    assert_eq!(
        app_doc["status"]["sync"]["revision"],
        "a23f2a7b21f788c07d95ceaeb529e54b7ae3dbcd"
    );
    let resources = app_doc["status"]["resources"].as_array().unwrap();
    let kinds: Vec<&str> = resources
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["Deployment", "Service", "ConfigMap", "Ingress"]);
    assert_eq!(resources[0]["health"]["status"], "Degraded");
    assert!(resources[1].get("group").is_none());
    let images = app_doc["status"]["summary"]["images"].as_array().unwrap();
    assert!(images.contains(&json!("ghcr.io/lumen-retail/checkout-web:2.14.0-rc3")));
    assert_eq!(
        app_doc["status"]["summary"]["externalURLs"],
        json!(["https://shop.lumen-retail.com/"])
    );
}

#[tokio::test]
async fn missing_applications_and_projects_are_grpc_not_found() {
    let a = app(&world(PAYMENTS));
    // Both payments-api apps live outside the default namespace.
    let (status, body) = call(&a, "GET", "/api/v1/applications/payments-api", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        json!({"error": "applications.argoproj.io \"payments-api\" not found", "code": 5, "message": "applications.argoproj.io \"payments-api\" not found"})
    );
    let (status, body) = call(
        &a,
        "GET",
        "/api/v1/applications/payments-api?appNamespace=payments-staging",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["spec"]["project"], "payments-staging");

    let (status, body) = call(&a, "GET", "/api/v1/projects/payments", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["spec"]["sourceNamespaces"],
        json!(["payments", "argocd"])
    );
    let (status, body) = call(&a, "GET", "/api/v1/projects/nope", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body["message"],
        "appprojects.argoproj.io \"nope\" not found"
    );

    let (status, body) = call(&a, "GET", "/api/v1/nothing", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        json!({"error": "Not Found", "code": 5, "message": "Not Found"})
    );
    let (status, _) = call(&a, "PATCH", "/api/v1/clusters", "").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn lists_clusters_with_filters_and_app_counts() {
    let a = app(&world(DEGRADED));
    let (_, all) = call(&a, "GET", "/api/v1/clusters", "").await;
    let items = all["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[1]["name"], "prod-us-east-1");
    assert_eq!(items[1]["info"]["applicationsCount"], 3);
    assert_eq!(items[1]["connectionState"]["status"], "Successful");
    let (_, one) = call(
        &a,
        "GET",
        "/api/v1/clusters?server=https%3A%2F%2Fstaging-use1.k8s.lumen.internal",
        "",
    )
    .await;
    assert_eq!(one["items"].as_array().unwrap().len(), 1);
    assert_eq!(one["items"][0]["info"]["applicationsCount"], 1);
    let (_, none) = call(&a, "GET", "/api/v1/clusters?name=eu-west", "").await;
    assert_eq!(none["items"], json!([]));
}

#[tokio::test]
async fn resource_tree_managed_resources_and_manifests() {
    let a = app(&world(DEGRADED));
    let (_, tree) = call(
        &a,
        "GET",
        "/api/v1/applications/checkout-web/resource-tree",
        "",
    )
    .await;
    let nodes = tree["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 10);
    let bad = nodes
        .iter()
        .find(|n| n["name"] == "checkout-web-7f8b9c6d5-q2x7m")
        .unwrap();
    assert_eq!(bad["kind"], "Pod");
    assert!(bad.get("group").is_none());
    assert_eq!(bad["parentRefs"][0]["kind"], "ReplicaSet");
    assert_eq!(
        bad["info"][0],
        json!({"name": "Status Reason", "value": "ImagePullBackOff"})
    );
    assert!(bad.get("manifest").is_none() && bad.get("app").is_none());

    let (_, managed) = call(
        &a,
        "GET",
        "/api/v1/applications/checkout-web/managed-resources?kind=ConfigMap",
        "",
    )
    .await;
    let items = managed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let live: Value = serde_json::from_str(items[0]["liveState"].as_str().unwrap()).unwrap();
    assert_eq!(live["data"]["LOG_LEVEL"], "info");
    assert_eq!(items[0]["modified"], false);

    let (status, res) = call(
        &a,
        "GET",
        "/api/v1/applications/checkout-web/resource?appNamespace=argocd&namespace=storefront&resourceName=checkout-web&group=apps&kind=Deployment&version=v1",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let manifest: Value = serde_json::from_str(res["manifest"].as_str().unwrap()).unwrap();
    assert_eq!(manifest["kind"], "Deployment");
    assert_eq!(manifest["spec"]["replicas"], 3);

    let (status, err) = call(
        &a,
        "GET",
        "/api/v1/applications/checkout-web/resource?namespace=storefront&resourceName=nope&group=apps&kind=Deployment&version=v1",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["code"], 3);
    assert_eq!(
        err["message"],
        "Deployment apps nope not found as part of application checkout-web"
    );
}

#[tokio::test]
async fn application_and_resource_events() {
    let a = app(&world(DEGRADED));
    let (_, app_events) = call(&a, "GET", "/api/v1/applications/checkout-web/events", "").await;
    let reasons: Vec<&str> = app_events["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        reasons,
        [
            "OperationStarted",
            "OperationCompleted",
            "ResourceUpdated",
            "ResourceUpdated"
        ]
    );
    let (_, pod_events) = call(
        &a,
        "GET",
        "/api/v1/applications/checkout-web/events?appNamespace=argocd&resourceNamespace=storefront&resourceUID=0d3df349-e40e-5789-bc1f-b9740a79c8a0&resourceName=checkout-web-7f8b9c6d5-q2x7m",
        "",
    )
    .await;
    let items = pod_events["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    assert!(
        items[2]["message"]
            .as_str()
            .unwrap()
            .contains("checkout-web:2.14.0-rc3: not found")
    );
    assert_eq!(items[2]["type"], "Warning");
}

#[tokio::test]
async fn streams_workload_logs_as_ndjson() {
    let a = app(&world(RESTART));
    let (status, text) = raw(
        &a,
        "GET",
        "/api/v1/applications/search-indexer/logs?appNamespace=argocd&namespace=search&resourceName=search-indexer-worker&group=apps&kind=Deployment&version=v1&follow=false&tailLines=2&container=search-indexer-worker",
        "",
        Some("argocd-toolbench"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    // 3 worker pods x 2 tail lines, then the `last` marker.
    assert_eq!(lines.len(), 7);
    assert!(text.ends_with('\n'));
    assert_eq!(
        lines[0]["result"]["podName"],
        "search-indexer-worker-85c7b6d9f-m7n8p"
    );
    assert!(
        lines[0]["result"]["content"]
            .as_str()
            .unwrap()
            .contains("unable to authenticate")
    );
    assert_eq!(lines[6]["result"]["last"], true);

    let (_, text) = raw(
        &a,
        "GET",
        "/api/v1/applications/search-indexer/pods/search-indexer-api-6d8f9b7c4-f3g4h/logs?follow=false&tailLines=100",
        "",
        Some("argocd-toolbench"),
    )
    .await;
    assert_eq!(text.lines().count(), 2);

    let (status, text) = raw(
        &a,
        "GET",
        "/api/v1/applications/nope/logs?follow=false",
        "",
        Some("argocd-toolbench"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let chunk: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(chunk["error"]["grpc_code"], 5);
    assert_eq!(chunk["error"]["http_code"], 404);
}

#[tokio::test]
async fn lists_and_runs_resource_actions() {
    let w = world(RESTART);
    let a = app(&w);
    let q = "appNamespace=argocd&namespace=search&resourceName=search-indexer-worker&group=apps&kind=Deployment&version=v1";
    let (status, actions) = call(
        &a,
        "GET",
        &format!("/api/v1/applications/search-indexer/resource/actions?{q}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        actions["actions"][0],
        json!({"name": "restart", "disabled": false, "iconClass": "fa fa-fw fa-redo", "displayName": "Restart"})
    );
    assert_eq!(actions["actions"][2]["name"], "resume");
    assert_eq!(actions["actions"][2]["disabled"], true);

    let path = format!("/api/v1/applications/search-indexer/resource/actions?{q}");
    let (status, err) = call(&a, "POST", &path, r#""resume""#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["message"], "action resume is disabled");
    let (status, err) = call(&a, "POST", &path, r#""explode""#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["code"], 3);

    let (status, body) = call(&a, "POST", &path, r#""restart""#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({}));
    let s = state(&w);
    let annotations =
        &s["resources"][WORKER_UID]["manifest"]["spec"]["template"]["metadata"]["annotations"];
    assert_eq!(
        annotations["kubectl.kubernetes.io/restartedAt"],
        "2026-10-29T14:05:21Z"
    );
    let events: Vec<&Value> = s["events"].as_object().unwrap().values().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0]["message"],
        "admin ran action restart on resource apps/Deployment/search-indexer-worker"
    );
    // A ConfigMap has no actions.
    let (_, none) = call(
        &a,
        "GET",
        "/api/v1/applications/search-indexer/resource/actions?namespace=search&resourceName=search-indexer-config&group=&kind=ConfigMap&version=v1",
        "",
    )
    .await;
    assert_eq!(none, json!({"actions": []}));
}

#[tokio::test]
async fn sync_with_prune_deletes_orphans_and_records_history() {
    let w = world(PAYMENTS);
    let a = app(&w);
    let (status, synced) = call(
        &a,
        "POST",
        "/api/v1/applications/payments-api/sync",
        r#"{"appNamespace":"payments","prune":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sha = "fe01f33d2a5392fa17c1c35f25d294d24075c28b";
    assert_eq!(synced["status"]["sync"]["status"], "Synced");
    assert_eq!(synced["status"]["sync"]["revision"], sha);
    let op = &synced["status"]["operationState"];
    assert_eq!(op["phase"], "Succeeded");
    assert_eq!(
        op["operation"]["sync"],
        json!({"revision": sha, "prune": true})
    );
    assert_eq!(op["operation"]["initiatedBy"], json!({"username": "admin"}));
    let results = op["syncResult"]["resources"].as_array().unwrap();
    let pruned = results
        .iter()
        .find(|r| r["name"] == "payments-api-legacy-webhooks")
        .unwrap();
    assert_eq!(pruned["status"], "Pruned");
    assert_eq!(
        results[0]["message"],
        "deployment.apps/payments-api configured"
    );
    let history = synced["status"]["history"].as_array().unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[2]["id"], 2);
    assert_eq!(history[2]["revision"], sha);
    assert_eq!(history[2]["deployedAt"], "2026-10-29T14:15:07Z");

    let s = state(&w);
    assert!(s["resources"].get(LEGACY_PROD_UID).is_none());
    assert!(s["resources"].get(LEGACY_STAGING_UID).is_some());
    let deployment = s["resources"]
        .as_object()
        .unwrap()
        .values()
        .find(|r| r["app"] == "payments/payments-api" && r["kind"] == "Deployment")
        .unwrap();
    assert_eq!(deployment["status"], "Synced");
    assert_eq!(
        deployment["manifest"]["spec"]["template"]["spec"]["containers"][0]["image"],
        "ghcr.io/lumen-retail/payments-api:3.18.0"
    );
    assert_eq!(
        s["applications"]["payments-staging/payments-api"]["status"]["sync"]["status"],
        "OutOfSync"
    );
}

#[tokio::test]
async fn sync_without_prune_stays_out_of_sync() {
    let w = world(PAYMENTS);
    let a = app(&w);
    let (_, synced) = call(
        &a,
        "POST",
        "/api/v1/applications/payments-api/sync",
        r#"{"appNamespace":"payments"}"#,
    )
    .await;
    assert_eq!(synced["status"]["sync"]["status"], "OutOfSync");
    let skipped = synced["status"]["operationState"]["syncResult"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "payments-api-legacy-webhooks")
        .unwrap()
        .clone();
    assert_eq!(skipped["status"], "PruneSkipped");
    assert_eq!(skipped["message"], "ignored (requires pruning)");
    assert!(state(&w)["resources"].get(LEGACY_PROD_UID).is_some());

    // Dry run changes nothing but the operation state.
    let w = world(PAYMENTS);
    let a = app(&w);
    let (_, dry) = call(
        &a,
        "POST",
        "/api/v1/applications/payments-api/sync",
        r#"{"appNamespace":"payments","prune":true,"dryRun":true}"#,
    )
    .await;
    assert_eq!(dry["status"]["history"].as_array().unwrap().len(), 2);
    assert_eq!(dry["status"]["sync"]["status"], "OutOfSync");
    assert!(state(&w)["resources"].get(LEGACY_PROD_UID).is_some());
}

#[tokio::test]
async fn sync_errors_match_argocd() {
    let a = app(&world(ROLLBACK));
    let (status, err) = call(
        &a,
        "POST",
        "/api/v1/applications/billing-worker/sync",
        r#"{"revision":"v1.42.0"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        err,
        json!({"error": "Cannot sync to v1.42.0: auto-sync currently set to main", "code": 9, "message": "Cannot sync to v1.42.0: auto-sync currently set to main"})
    );
    let (status, err) = call(&a, "POST", "/api/v1/applications/ghost/sync", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["code"], 5);

    let a = app(&world(PAYMENTS));
    let (status, err) = call(
        &a,
        "POST",
        "/api/v1/applications/payments-api/sync",
        r#"{"appNamespace":"payments","revision":"v9.9.9"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        err["message"],
        "rpc error: code = Unknown desc = unable to resolve 'v9.9.9' to a commit SHA"
    );
}

fn billing_worker_body(target: &str, automated: bool) -> String {
    let mut sync_policy = json!({
        "syncOptions": ["CreateNamespace=true", "ApplyOutOfSyncOnly=true"],
        "retry": {"limit": 5, "backoff": {"duration": "5s", "maxDuration": "3m0s", "factor": 2}}
    });
    if automated {
        sync_policy["automated"] = json!({"prune": true, "selfHeal": true});
    }
    json!({
        "metadata": {"name": "billing-worker", "namespace": "argocd"},
        "spec": {
            "project": "billing",
            "source": {"repoURL": "https://github.com/lumen-retail/billing-worker-deploy.git", "path": "overlays/production", "targetRevision": target},
            "syncPolicy": sync_policy,
            "destination": {"server": "https://prod-use1.k8s.lumen.internal", "namespace": "billing"}
        }
    })
    .to_string()
}

#[tokio::test]
async fn update_with_auto_sync_rolls_back() {
    let w = world(ROLLBACK);
    let a = app(&w);
    let (status, updated) = call(
        &a,
        "PUT",
        "/api/v1/applications/billing-worker",
        &billing_worker_body("v1.42.0", true),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let b = "b0826e986c9c73e0088c6ecb2a14458d247d2205";
    assert_eq!(updated["spec"]["source"]["targetRevision"], "v1.42.0");
    assert_eq!(updated["status"]["sync"]["status"], "Synced");
    assert_eq!(updated["status"]["sync"]["revision"], b);
    assert_eq!(updated["status"]["health"]["status"], "Healthy");
    let op = &updated["status"]["operationState"];
    assert_eq!(op["operation"]["initiatedBy"], json!({"automated": true}));
    assert_eq!(op["syncResult"]["revision"], b);
    assert_eq!(updated["metadata"]["generation"], 7);
    // Update replaces labels with the request's (none here).
    assert!(updated["metadata"].get("labels").is_none());
    let reasons: Vec<String> = state(&w)["events"]
        .as_object()
        .unwrap()
        .values()
        .filter(|e| e["involvedObject"]["name"] == "billing-worker")
        .map(|e| e["message"].as_str().unwrap().to_owned())
        .collect();
    assert!(reasons.contains(&"admin updated application spec".to_owned()));
    assert!(reasons.contains(&"Updated health status: Degraded -> Healthy".to_owned()));
}

#[tokio::test]
async fn update_without_auto_sync_goes_out_of_sync_then_syncs() {
    let w = world(ROLLBACK);
    let a = app(&w);
    let (_, updated) = call(
        &a,
        "PUT",
        "/api/v1/applications/billing-worker",
        &billing_worker_body("v1.42.0", false),
    )
    .await;
    assert_eq!(updated["status"]["sync"]["status"], "OutOfSync");
    assert_eq!(updated["status"]["history"].as_array().unwrap().len(), 3);
    let (status, synced) = call(&a, "POST", "/api/v1/applications/billing-worker/sync", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(synced["status"]["sync"]["status"], "Synced");
    assert_eq!(synced["status"]["history"][3]["id"], 3);

    let (status, err) = call(
        &a,
        "PUT",
        "/api/v1/applications/billing-worker",
        &billing_worker_body("v0.0.1", false),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .ends_with("unable to resolve 'v0.0.1' to a commit SHA")
    );
    let (status, _) = call(
        &a,
        "PUT",
        "/api/v1/applications/ghost",
        &billing_worker_body("main", false),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn creates_validates_and_deletes_applications() {
    let w = world(ROLLBACK);
    let a = app(&w);
    let body = json!({
        "metadata": {"name": "billing-worker-canary", "namespace": "argocd"},
        "spec": {
            "project": "billing",
            "source": {"repoURL": "https://github.com/lumen-retail/billing-worker-deploy.git", "path": "overlays/canary", "targetRevision": "v1.42.0"},
            "syncPolicy": {"syncOptions": [], "retry": {"limit": 1, "backoff": {"duration": "5s", "maxDuration": "1m", "factor": 2}}},
            "destination": {"name": "prod-us-east-1", "namespace": "billing"}
        }
    });
    let (status, created) = call(&a, "POST", "/api/v1/applications", &body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["metadata"]["generation"], 1);
    assert_eq!(
        created["metadata"]["creationTimestamp"],
        "2026-10-29T14:05:07Z"
    );
    assert_eq!(created["status"]["sync"]["status"], "OutOfSync");
    assert_eq!(created["status"]["health"]["status"], "Missing");
    assert!(
        state(&w)["applications"]
            .get("argocd/billing-worker-canary")
            .is_some()
    );
    // Same spec again: idempotent. Different spec: rejected without upsert.
    let (status, _) = call(&a, "POST", "/api/v1/applications", &body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    let mut changed = body.clone();
    changed["spec"]["source"]["targetRevision"] = json!("main");
    let (status, err) = call(&a, "POST", "/api/v1/applications", &changed.to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        err["message"],
        "existing application spec is different, use upsert flag to force update"
    );

    let mut bad = body.clone();
    bad["spec"]["project"] = json!("nope");
    let (_, err) = call(&a, "POST", "/api/v1/applications", &bad.to_string()).await;
    assert_eq!(
        err["message"],
        "application spec for billing-worker-canary is invalid: InvalidSpecError: application references project nope which does not exist"
    );
    let mut bad = body.clone();
    bad["spec"]["destination"]["namespace"] = json!("kube-system");
    let (status, err) = call(&a, "POST", "/api/v1/applications", &bad.to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("do not match any of the allowed destinations in project 'billing'")
    );
    let mut bad = body.clone();
    bad["spec"]["source"]["repoURL"] = json!("https://gitlab.com/someone/else.git");
    let (_, err) = call(&a, "POST", "/api/v1/applications", &bad.to_string()).await;
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("is not permitted in project 'billing'")
    );
    let (status, err) = call(&a, "POST", "/api/v1/applications", "{nope").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["code"], 3);

    // Cascade delete removes the app and its resources; orphan delete keeps them.
    let (status, body) = call(
        &a,
        "DELETE",
        "/api/v1/applications/billing-api?cascade=true",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({}));
    let (status, body) = call(
        &a,
        "DELETE",
        "/api/v1/applications/billing-worker-staging?cascade=false",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let s = state(&w);
    assert!(s["applications"].get("argocd/billing-api").is_none());
    let resources = s["resources"].as_object().unwrap();
    assert!(!resources.values().any(|r| r["app"] == "argocd/billing-api"));
    assert!(
        resources
            .values()
            .any(|r| r["app"].is_null() && r["name"] == "billing-worker")
    );
    let (status, _) = call(&a, "DELETE", "/api/v1/applications/billing-api", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
