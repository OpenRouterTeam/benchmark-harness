use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_kubernetes::{Kubernetes, util::b64_decode};
use serde_json::{Value, json};
use tower::ServiceExt;

const CRASHLOOP: &str = "crashloop-root-cause";
const HELM: &str = "uninstall-finished-loadtest";
const BAD_POD: &str = "ledger-worker-zx4mrndj8b-gdg8j";

fn world(slug: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/ops/kubernetes")
        .join(slug);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
    text: String,
}

async fn send(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)], body: &str) -> Resp {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8115");
    let mut has_ct = false;
    for (k, v) in headers {
        has_ct |= k.eq_ignore_ascii_case("content-type");
        req = req.header(*k, *v);
    }
    if !has_ct {
        req = req.header("content-type", "application/json");
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn get(app: &Router, uri: &str) -> Resp {
    send(app, "GET", uri, &[], "").await
}

fn names(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["metadata"]["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn version_and_legacy_discovery() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let v = get(&app, "/version").await;
    assert_eq!(v.body["gitVersion"], "v1.33.4");

    let api = get(&app, "/api").await;
    assert_eq!(api.body["kind"], "APIVersions");
    assert_eq!(api.body["versions"], json!(["v1"]));

    let apis = get(&app, "/apis").await;
    let groups: Vec<&str> = apis.body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["name"].as_str().unwrap())
        .collect();
    assert!(groups.contains(&"apps") && groups.contains(&"metrics.k8s.io"));
    let autoscaling = apis.body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == "autoscaling")
        .unwrap();
    assert_eq!(autoscaling["preferredVersion"]["version"], "v2");

    let core = get(&app, "/api/v1").await;
    let resources: Vec<&str> = core.body["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(resources.contains(&"pods") && resources.contains(&"pods/log"));

    let apps = get(&app, "/apis/apps/v1").await;
    let scale = apps.body["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "deployments/scale")
        .unwrap();
    assert_eq!(scale["kind"], "Scale");
    assert_eq!(scale["group"], "autoscaling");

    let missing = get(&app, "/apis/tekton.dev/v1/namespaces/x/pipelineruns").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.body["kind"], "Status");
    assert_eq!(missing.body["reason"], "NotFound");
    assert_eq!(
        missing.body["message"],
        "the server could not find the requested resource"
    );
}

#[tokio::test]
async fn wrong_token_is_unauthorized() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let r = send(
        &app,
        "GET",
        "/api/v1/namespaces",
        &[("authorization", "Bearer nope")],
        "",
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.body["reason"], "Unauthorized");
    let ok = send(
        &app,
        "GET",
        "/api/v1/namespaces",
        &[("authorization", "Bearer k8s-toolbench")],
        "",
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
}

#[tokio::test]
async fn lists_with_selectors_and_paging() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let pods = get(&app, "/api/v1/namespaces/payments/pods").await;
    assert_eq!(pods.body["kind"], "PodList");
    assert_eq!(pods.body["apiVersion"], "v1");
    let all = names(&pods.body);
    assert_eq!(all.len(), 6);
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(all, sorted, "lists are ordered by key like etcd");
    assert!(
        pods.body["items"][0].get("kind").is_none(),
        "built-in list items omit type meta"
    );

    let ledger = get(
        &app,
        "/api/v1/pods?labelSelector=app.kubernetes.io%2Fname%3Dledger-worker",
    )
    .await;
    assert_eq!(names(&ledger.body).len(), 3);
    let in_set = get(
        &app,
        "/api/v1/namespaces/payments/pods?labelSelector=app.kubernetes.io%2Fname+in+%28ledger-worker%2Crefund-scheduler%29",
    )
    .await;
    assert_eq!(names(&in_set.body).len(), 3);
    let by_field = get(
        &app,
        "/api/v1/namespaces/payments/events?fieldSelector=involvedObject.name%3Dledger-worker-zx4mrndj8b-gdg8j%2Ctype%3DWarning",
    )
    .await;
    assert_eq!(names(&by_field.body).len(), 1);
    let bad = get(&app, "/api/v1/pods?fieldSelector=nonsense").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    let page1 = get(&app, "/api/v1/namespaces/payments/pods?limit=4").await;
    assert_eq!(names(&page1.body), all[..4]);
    assert_eq!(page1.body["metadata"]["remainingItemCount"], 2);
    let token = page1.body["metadata"]["continue"].as_str().unwrap();
    let page2 = get(
        &app,
        &format!("/api/v1/namespaces/payments/pods?limit=4&continue={token}"),
    )
    .await;
    assert_eq!(names(&page2.body), all[4..]);
    assert!(page2.body["metadata"].get("continue").is_none());
}

#[tokio::test]
async fn table_output_matches_kubectl_columns() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let t = send(
        &app,
        "GET",
        "/api/v1/namespaces/payments/pods",
        &[(
            "accept",
            "application/json;as=Table;v=v1;g=meta.k8s.io,application/json",
        )],
        "",
    )
    .await;
    assert_eq!(t.body["kind"], "Table");
    let cols: Vec<&str> = t.body["columnDefinitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(&cols[..5], ["Name", "Ready", "Status", "Restarts", "Age"]);
    let row = t.body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["cells"][0] == BAD_POD)
        .unwrap();
    assert_eq!(row["cells"][1], "0/1");
    assert_eq!(row["cells"][2], "CrashLoopBackOff");
    assert_eq!(row["cells"][3], "14 (6m16s ago)");
    assert_eq!(row["object"]["kind"], "PartialObjectMetadata");
    assert_eq!(row["object"]["metadata"]["namespace"], "payments");
}

#[tokio::test]
async fn not_found_status_shape() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let r = get(&app, "/apis/apps/v1/namespaces/payments/deployments/nope").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        r.body,
        json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
            "message": "deployments.apps \"nope\" not found", "reason": "NotFound",
            "details": {"name": "nope", "group": "apps", "kind": "deployments"}, "code": 404
        })
    );
}

#[tokio::test]
async fn create_conflict_and_validation() {
    let w = world(CRASHLOOP);
    let app = service_app(w.clone(), &Kubernetes);
    let cm = r#"{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"ledger-flags"},"data":{"RETRY":"3"}}"#;
    let created = send(
        &app,
        "POST",
        "/api/v1/namespaces/payments/configmaps",
        &[],
        cm,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.body["metadata"]["namespace"], "payments");
    assert_eq!(
        created.body["metadata"]["creationTimestamp"],
        "2026-11-03T08:40:07Z"
    );
    assert!(created.body["metadata"]["uid"].as_str().unwrap().len() == 36);

    let again = send(
        &app,
        "POST",
        "/api/v1/namespaces/payments/configmaps",
        &[],
        cm,
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.body["reason"], "AlreadyExists");
    assert_eq!(
        again.body["message"],
        "configmaps \"ledger-flags\" already exists"
    );

    let no_ns = send(&app, "POST", "/api/v1/namespaces/ghost/configmaps", &[], cm).await;
    assert_eq!(no_ns.status, StatusCode::NOT_FOUND);
    assert_eq!(no_ns.body["message"], "namespaces \"ghost\" not found");

    let bad_name = send(
        &app,
        "POST",
        "/api/v1/namespaces/payments/configmaps",
        &[],
        r#"{"metadata":{"name":"Bad_Name"}}"#,
    )
    .await;
    assert_eq!(bad_name.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(bad_name.body["reason"], "Invalid");

    let no_containers = send(
        &app,
        "POST",
        "/api/v1/namespaces/payments/pods",
        &[],
        r#"{"metadata":{"name":"p"},"spec":{}}"#,
    )
    .await;
    assert_eq!(no_containers.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        no_containers.body["message"],
        "Pod \"p\" is invalid: spec.containers: Required value"
    );

    let state = w.lock().unwrap().snapshot();
    assert_eq!(
        state["services"]["kubernetes"]["configmaps"]["payments/ledger-flags"]["data"]["RETRY"],
        "3"
    );
}

#[tokio::test]
async fn patch_flavours_and_server_side_apply() {
    let w = world(CRASHLOOP);
    let app = service_app(w.clone(), &Kubernetes);
    let path = "/apis/apps/v1/namespaces/payments/deployments/ledger-worker";
    let merge = send(
        &app,
        "PATCH",
        path,
        &[("content-type", "application/merge-patch+json")],
        r#"{"metadata":{"labels":{"tier":"backend"}}}"#,
    )
    .await;
    assert_eq!(merge.status, StatusCode::OK);
    assert_eq!(merge.body["metadata"]["labels"]["tier"], "backend");
    assert_eq!(
        merge.body["metadata"]["generation"], 7,
        "metadata-only change keeps generation"
    );

    let apply = send(
        &app,
        "PATCH",
        &format!("{path}?fieldManager=kubernetes-mcp-server&force=true"),
        &[("content-type", "application/apply-patch+yaml")],
        r#"{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"ledger-worker","namespace":"payments"},"spec":{"template":{"spec":{"containers":[{"name":"ledger-worker","resources":{"limits":{"memory":"512Mi"}}}]}}}}"#,
    )
    .await;
    assert_eq!(apply.status, StatusCode::OK);
    let c = &apply.body["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(c["resources"]["limits"]["memory"], "512Mi");
    assert_eq!(
        c["image"], "ghcr.io/octo-corp/ledger-worker:1.9.0",
        "apply merges containers by name"
    );
    assert_eq!(apply.body["metadata"]["generation"], 8);

    let jp = send(
        &app,
        "PATCH",
        path,
        &[("content-type", "application/json-patch+json")],
        r#"[{"op":"replace","path":"/spec/replicas","value":3}]"#,
    )
    .await;
    assert_eq!(jp.body["spec"]["replicas"], 3);
    let bad_jp = send(
        &app,
        "PATCH",
        path,
        &[("content-type", "application/json-patch+json")],
        r#"[{"op":"replace","path":"/spec/nothing","value":3}]"#,
    )
    .await;
    assert_eq!(bad_jp.status, StatusCode::UNPROCESSABLE_ENTITY);

    let created = send(
        &app,
        "PATCH",
        "/api/v1/namespaces/payments/configmaps/new-cm?fieldManager=x",
        &[("content-type", "application/apply-patch+yaml")],
        r#"{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"new-cm"},"data":{"a":"1"}}"#,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);

    let missing = send(
        &app,
        "PATCH",
        "/apis/apps/v1/namespaces/payments/deployments/nope",
        &[("content-type", "application/merge-patch+json")],
        "{}",
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    let snap = w.lock().unwrap().snapshot();
    let dep = &snap["services"]["kubernetes"]["deployments.apps"]["payments/ledger-worker"];
    assert_eq!(dep["spec"]["replicas"], 3);
    assert_eq!(dep["metadata"]["labels"]["tier"], "backend");
}

#[tokio::test]
async fn update_checks_resource_version() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let path = "/api/v1/namespaces/payments/services/ledger-worker";
    let mut svc = get(&app, path).await.body;
    svc["metadata"]["labels"]["x"] = json!("y");
    let ok = send(&app, "PUT", path, &[], &svc.to_string()).await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_ne!(
        ok.body["metadata"]["resourceVersion"],
        svc["metadata"]["resourceVersion"]
    );
    let stale = send(&app, "PUT", path, &[], &svc.to_string()).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.body["reason"], "Conflict");
}

#[tokio::test]
async fn scale_subresource_updates_deployment() {
    let w = world(CRASHLOOP);
    let app = service_app(w.clone(), &Kubernetes);
    let path = "/apis/apps/v1/namespaces/payments/deployments/payments-api/scale";
    let scale = get(&app, path).await;
    assert_eq!(scale.body["kind"], "Scale");
    assert_eq!(scale.body["apiVersion"], "autoscaling/v1");
    assert_eq!(scale.body["spec"]["replicas"], 3);
    assert_eq!(
        scale.body["status"]["selector"],
        "app.kubernetes.io/name=payments-api"
    );

    let mut next = scale.body.clone();
    next["spec"]["replicas"] = json!(6);
    let put = send(&app, "PUT", path, &[], &next.to_string()).await;
    assert_eq!(put.status, StatusCode::OK);
    assert_eq!(put.body["spec"]["replicas"], 6);
    let negative = send(
        &app,
        "PATCH",
        path,
        &[("content-type", "application/merge-patch+json")],
        r#"{"spec":{"replicas":-1}}"#,
    )
    .await;
    assert_eq!(negative.status, StatusCode::UNPROCESSABLE_ENTITY);
    let snap = w.lock().unwrap().snapshot();
    let dep = &snap["services"]["kubernetes"]["deployments.apps"]["payments/payments-api"];
    assert_eq!(dep["spec"]["replicas"], 6);
    assert_eq!(dep["metadata"]["generation"], 2);
}

#[tokio::test]
async fn pod_logs_previous_tail_and_errors() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let base = format!("/api/v1/namespaces/payments/pods/{BAD_POD}/log");
    let prev = get(&app, &format!("{base}?previous=true")).await;
    assert_eq!(prev.status, StatusCode::OK);
    assert!(
        prev.text
            .contains("password authentication failed for user \"ledger\"")
    );
    let tail = get(&app, &format!("{base}?tailLines=1")).await;
    assert_eq!(tail.text.lines().count(), 1);
    assert!(tail.text.contains("FATAL"));
    let bad_container = get(&app, &format!("{base}?container=sidecar")).await;
    assert_eq!(bad_container.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        bad_container.body["message"],
        format!("container sidecar is not valid for pod {BAD_POD}")
    );
    let healthy = get(
        &app,
        "/api/v1/namespaces/payments/pods/ledger-worker-zx4mrndj8b-dwvg9/log?previous=true",
    )
    .await;
    assert_eq!(healthy.status, StatusCode::BAD_REQUEST);
    let missing = get(&app, "/api/v1/namespaces/payments/pods/nope/log").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_a_replicaset_pod_gets_it_replaced() {
    let w = world(CRASHLOOP);
    let app = service_app(w.clone(), &Kubernetes);
    let del = send(
        &app,
        "DELETE",
        &format!("/api/v1/namespaces/payments/pods/{BAD_POD}"),
        &[],
        "",
    )
    .await;
    assert_eq!(del.status, StatusCode::OK);
    assert!(del.body["metadata"]["deletionTimestamp"].is_string());
    let pods = get(
        &app,
        "/api/v1/namespaces/payments/pods?labelSelector=app.kubernetes.io%2Fname%3Dledger-worker",
    )
    .await;
    let n = names(&pods.body);
    assert_eq!(n.len(), 2);
    assert!(!n.contains(&BAD_POD.to_owned()));
    let replacement = pods.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["metadata"]["name"] != "ledger-worker-zx4mrndj8b-dwvg9")
        .unwrap();
    assert_eq!(replacement["status"]["phase"], "Pending");
    assert!(
        replacement["metadata"]["name"]
            .as_str()
            .unwrap()
            .starts_with("ledger-worker-zx4mrndj8b-")
    );
    let again = send(
        &app,
        "DELETE",
        &format!("/api/v1/namespaces/payments/pods/{BAD_POD}"),
        &[],
        "",
    )
    .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_a_deployment_cascades() {
    let w = world(CRASHLOOP);
    let app = service_app(w.clone(), &Kubernetes);
    let del = send(
        &app,
        "DELETE",
        "/apis/apps/v1/namespaces/payments/deployments/payments-api",
        &[],
        r#"{"propagationPolicy":"Background"}"#,
    )
    .await;
    assert_eq!(del.status, StatusCode::OK);
    let rs = get(&app, "/apis/apps/v1/namespaces/payments/replicasets").await;
    assert!(
        names(&rs.body)
            .iter()
            .all(|n| !n.starts_with("payments-api"))
    );
    let pods = get(&app, "/api/v1/namespaces/payments/pods").await;
    assert!(
        names(&pods.body)
            .iter()
            .all(|n| !n.starts_with("payments-api"))
    );
    assert_eq!(names(&pods.body).len(), 3);
}

#[tokio::test]
async fn access_reviews_allow_all_but_exec() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let path = "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews";
    let list = send(
        &app,
        "POST",
        path,
        &[],
        r#"{"spec":{"resourceAttributes":{"verb":"list","resource":"pods"}}}"#,
    )
    .await;
    assert_eq!(list.status, StatusCode::CREATED);
    assert_eq!(list.body["status"]["allowed"], true);
    let exec = send(&app, "POST", path, &[], r#"{"spec":{"resourceAttributes":{"verb":"create","resource":"pods","subresource":"exec"}}}"#).await;
    assert_eq!(exec.body["status"]["allowed"], false);
    let forbidden = send(
        &app,
        "POST",
        &format!("/api/v1/namespaces/payments/pods/{BAD_POD}/exec?command=ls"),
        &[],
        "",
    )
    .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    assert_eq!(forbidden.body["reason"], "Forbidden");
}

#[tokio::test]
async fn protobuf_access_review_from_typed_clients() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    // client-go typed SelfSubjectAccessReview{namespace: payments, verb: list, version: v1, resource: pods}
    let mut body = b"k8s\0".to_vec();
    let field = |n: u8, b: &[u8]| {
        let mut v = vec![(n << 3) | 2, b.len() as u8];
        v.extend_from_slice(b);
        v
    };
    let ra = [
        field(1, b"payments"),
        field(2, b"list"),
        field(4, b"v1"),
        field(5, b"pods"),
    ]
    .concat();
    let raw = [field(1, b""), field(2, &field(1, &ra))].concat();
    let tm = [
        field(1, b"authorization.k8s.io/v1"),
        field(2, b"SelfSubjectAccessReview"),
    ]
    .concat();
    body.extend([field(1, &tm), field(2, &raw)].concat());
    let req = Request::builder()
        .method("POST")
        .uri("/apis/authorization.k8s.io/v1/selfsubjectaccessreviews")
        .header("content-type", "application/vnd.kubernetes.protobuf")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(v["spec"]["resourceAttributes"]["namespace"], "payments");
    assert_eq!(v["status"]["allowed"], true);
}

#[tokio::test]
async fn metrics_and_node_proxy() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let pm = get(
        &app,
        "/apis/metrics.k8s.io/v1beta1/namespaces/payments/pods",
    )
    .await;
    assert_eq!(pm.body["kind"], "PodMetricsList");
    assert_eq!(names(&pm.body).len(), 4);
    assert!(pm.body["items"][0]["timestamp"].is_string());
    let nm = get(
        &app,
        "/apis/metrics.k8s.io/v1beta1/nodes/ip-10-0-1-21.ec2.internal",
    )
    .await;
    assert_eq!(nm.body["usage"]["cpu"], "1873m");
    let ro = send(
        &app,
        "DELETE",
        "/apis/metrics.k8s.io/v1beta1/nodes/ip-10-0-1-21.ec2.internal",
        &[],
        "",
    )
    .await;
    assert_eq!(ro.status, StatusCode::METHOD_NOT_ALLOWED);

    let stats = get(
        &app,
        "/api/v1/nodes/ip-10-0-1-21.ec2.internal/proxy/stats/summary",
    )
    .await;
    assert_eq!(stats.body["node"]["nodeName"], "ip-10-0-1-21.ec2.internal");
    let logs = get(
        &app,
        "/api/v1/nodes/ip-10-0-1-21.ec2.internal/proxy/logs?query=kubelet&tailLines=2",
    )
    .await;
    assert_eq!(logs.status, StatusCode::OK);
    assert_eq!(logs.text.lines().count(), 2);
    let unknown = get(
        &app,
        "/api/v1/nodes/ip-10-0-1-21.ec2.internal/proxy/logs?query=nope",
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let no_node = get(&app, "/api/v1/nodes/ghost/proxy/stats/summary").await;
    assert_eq!(no_node.body["message"], "nodes \"ghost\" not found");
}

#[tokio::test]
async fn watch_streams_current_objects() {
    let app = service_app(world(CRASHLOOP), &Kubernetes);
    let r = get(&app, "/api/v1/namespaces/payments/services?watch=true").await;
    let events: Vec<Value> = r
        .text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "ADDED");
    assert_eq!(r.headers["content-type"], "application/json");
}

#[tokio::test]
async fn helm_releases_are_served_as_secrets() {
    let w = world(HELM);
    let app = service_app(w.clone(), &Kubernetes);
    let list = get(
        &app,
        "/api/v1/namespaces/perf/secrets?labelSelector=owner%3Dhelm",
    )
    .await;
    let n = names(&list.body);
    assert_eq!(
        n,
        [
            "sh.helm.release.v1.checkout-loadtest.v1",
            "sh.helm.release.v1.checkout-loadtest.v2",
            "sh.helm.release.v1.k6-operator.v3",
            "sh.helm.release.v1.search-loadtest.v1"
        ]
    );
    let secret = &list.body["items"][1];
    assert_eq!(secret["type"], "helm.sh/release.v1");
    assert_eq!(secret["metadata"]["labels"]["status"], "deployed");
    let outer = b64_decode(secret["data"]["release"].as_str().unwrap()).unwrap();
    let inner = b64_decode(std::str::from_utf8(&outer).unwrap()).unwrap();
    let release: Value = serde_json::from_slice(&inner).unwrap();
    assert_eq!(release["name"], "checkout-loadtest");
    assert_eq!(release["version"], 2);

    let del = send(
        &app,
        "DELETE",
        "/api/v1/namespaces/perf/secrets/sh.helm.release.v1.checkout-loadtest.v1",
        &[],
        "",
    )
    .await;
    assert_eq!(del.status, StatusCode::OK);
    let snap = w.lock().unwrap().snapshot();
    let rels = &snap["services"]["kubernetes"]["helm_releases"];
    assert!(rels.get("perf/checkout-loadtest.v1").is_none());
    assert!(rels.get("perf/checkout-loadtest.v2").is_some());
}

#[tokio::test]
async fn deleting_a_job_removes_its_pods() {
    let w = world(HELM);
    let app = service_app(w.clone(), &Kubernetes);
    let del = send(
        &app,
        "DELETE",
        "/apis/batch/v1/namespaces/perf/jobs/checkout-loadtest",
        &[],
        "",
    )
    .await;
    assert_eq!(del.status, StatusCode::OK);
    let pods = get(
        &app,
        "/api/v1/namespaces/perf/pods?labelSelector=job-name%3Dcheckout-loadtest",
    )
    .await;
    assert!(names(&pods.body).is_empty());
    let staging = get(
        &app,
        "/api/v1/namespaces/perf-staging/pods?labelSelector=job-name%3Dcheckout-loadtest",
    )
    .await;
    assert_eq!(names(&staging.body).len(), 1);
}

#[tokio::test]
async fn namespaces_cannot_delete_default_and_cascade_otherwise() {
    let w = world(HELM);
    let app = service_app(w.clone(), &Kubernetes);
    let forbidden = send(&app, "DELETE", "/api/v1/namespaces/default", &[], "").await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    let del = send(&app, "DELETE", "/api/v1/namespaces/perf-staging", &[], "").await;
    assert_eq!(del.body["status"]["phase"], "Terminating");
    let snap = w.lock().unwrap().snapshot();
    let k = &snap["services"]["kubernetes"];
    assert!(
        k["jobs.batch"]
            .get("perf-staging/checkout-loadtest")
            .is_none()
    );
    assert!(
        k["helm_releases"]
            .get("perf-staging/checkout-loadtest.v1")
            .is_none()
    );
}
