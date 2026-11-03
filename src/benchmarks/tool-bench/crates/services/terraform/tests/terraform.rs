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
use mock_terraform::Terraform;
use serde_json::{Value, json};
use tower::ServiceExt;

const CHECKOUT_STG: &str = "ws-FxCPzmnaeIMOUbSd";
const DESIRED_COUNT: &str = "var-4eRpJG8P0JYsXLTJ";
const NET_PROD: &str = "ws-X6Cm5W6LhJnKhlo2";
const NET_PROD_RUN: &str = "run-1q2oy8I2MqSd5oM4";
const NET_STG_RUN: &str = "run-blTtGlrNXHe62r7y";
const ORDERS_PROD: &str = "ws-gOHPQz7D1ZRgqBJq";
const ORDERS_LEGACY: &str = "ws-QMfGO0wb8mhyWgrq";
const PAY_PROD_FAILED_RUN: &str = "run-Gy4tBHGggKx49nbH";
const PAY_PROD_FAILED_APPLY: &str = "apply-OttY2qKt1DNXCd1P";
const PAY_PROD_FAILED_PLAN: &str = "plan-W6mMDz8dMca2TBnD";

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/code/terraform/scale-checkout-staging");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn empty_world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scenarios/example/github/basic");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
    raw: Vec<u8>,
}

async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>, auth: bool) -> Resp {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8109")
        .header("content-type", "application/vnd.api+json");
    if auth {
        req = req.header("authorization", "Bearer toolbench.atlasv1.token");
    }
    let req = req
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let raw = to_bytes(body, usize::MAX).await.unwrap().to_vec();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
        raw,
    }
}

async fn get(app: &Router, uri: &str) -> Resp {
    send(app, "GET", uri, None, true).await
}

fn ids(body: &Value) -> Vec<String> {
    body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_owned())
        .collect()
}

fn names(body: &Value) -> Vec<String> {
    body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["attributes"]["name"].as_str().unwrap().to_owned())
        .collect()
}

fn state(world: &SharedWorld) -> Value {
    world.lock().unwrap().snapshot()["services"]["terraform"].clone()
}

#[tokio::test]
async fn ping_reports_api_version_headers_like_tfe() {
    let app = service_app(empty_world(), &Terraform);
    let resp = send(&app, "GET", "/api/v2/ping", None, false).await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    assert_eq!(resp.headers["tfp-api-version"], "2.6");
    assert_eq!(resp.headers["tfp-appname"], "HCP Terraform");
}

#[tokio::test]
async fn api_requires_a_bearer_token_and_404s_are_jsonapi() {
    let app = service_app(world(), &Terraform);
    let resp = send(&app, "GET", "/api/v2/organizations", None, false).await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
    assert_eq!(resp.body["errors"][0]["status"], "401");

    let resp = get(&app, "/api/v2/organizations/nope").await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
    assert_eq!(resp.headers["content-type"], "application/vnd.api+json");
    assert_eq!(resp.body["errors"][0]["title"], "not found");
}

#[tokio::test]
async fn account_orgs_and_projects() {
    let app = service_app(world(), &Terraform);
    let me = get(&app, "/api/v2/account/details").await;
    assert_eq!(me.body["data"]["type"], "users");
    assert_eq!(me.body["data"]["attributes"]["username"], "sam-rivera");

    let orgs = get(
        &app,
        "/api/v2/organizations?page%5Bnumber%5D=1&page%5Bsize%5D=1",
    )
    .await;
    assert_eq!(ids(&orgs.body), ["acme-platform"]);
    assert_eq!(orgs.body["meta"]["pagination"]["total-count"], 2);
    assert_eq!(orgs.body["meta"]["pagination"]["next-page"], 2);

    let org = get(&app, "/api/v2/organizations/acme-platform").await;
    assert_eq!(
        org.body["data"]["attributes"]["permissions"]["can-create-team"],
        true
    );

    let projects = get(&app, "/api/v2/organizations/acme-platform/projects").await;
    assert_eq!(
        names(&projects.body),
        [
            "Commerce",
            "Default Project",
            "Legacy",
            "Networking",
            "Payments"
        ]
    );
    let commerce = &projects.body["data"][0];
    assert_eq!(commerce["attributes"]["workspace-count"], 4);
}

#[tokio::test]
async fn project_create_validates_and_delete_refuses_nonempty() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let body =
        |name: &str| json!({ "data": { "type": "projects", "attributes": { "name": name } } });
    let dup = send(
        &app,
        "POST",
        "/api/v2/organizations/acme-platform/projects",
        Some(body("Commerce")),
        true,
    )
    .await;
    assert_eq!(dup.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        dup.body["errors"][0]["detail"],
        "Name has already been taken"
    );

    let created = send(
        &app,
        "POST",
        "/api/v2/organizations/acme-platform/projects",
        Some(body("Data Platform")),
        true,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = created.body["data"]["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("prj-"));
    assert_eq!(state(&world)["projects"][&id]["name"], "Data Platform");

    let commerce = "prj-RGb4nwJ3c5kaUkvv";
    let refused = send(
        &app,
        "DELETE",
        &format!("/api/v2/projects/{commerce}"),
        None,
        true,
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    let deleted = send(
        &app,
        "DELETE",
        &format!("/api/v2/projects/{id}"),
        None,
        true,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(state(&world)["projects"].get(&id).is_none());
}

#[tokio::test]
async fn workspaces_list_filters_and_reads() {
    let app = service_app(world(), &Terraform);
    let all = get(
        &app,
        "/api/v2/organizations/acme-platform/workspaces?search%5Bname%5D=orders",
    )
    .await;
    assert_eq!(
        names(&all.body),
        [
            "orders-api-prod",
            "orders-api-prod-legacy",
            "orders-api-staging"
        ]
    );
    let commerce = get(
        &app,
        "/api/v2/organizations/acme-platform/workspaces?search%5Bname%5D=orders&filter%5Bproject%5D%5Bid%5D=prj-RGb4nwJ3c5kaUkvv",
    )
    .await;
    assert_eq!(
        names(&commerce.body),
        ["orders-api-prod", "orders-api-staging"]
    );
    let tagged = get(
        &app,
        "/api/v2/organizations/acme-platform/workspaces?search%5Btags%5D=legacy",
    )
    .await;
    assert_eq!(
        names(&tagged.body),
        ["billing-etl-prod", "orders-api-prod-legacy"]
    );
    let wildcard = get(
        &app,
        "/api/v2/organizations/acme-platform/workspaces?search%5Bwildcard-name%5D=*-staging",
    )
    .await;
    assert_eq!(wildcard.body["meta"]["pagination"]["total-count"], 4);

    let ws = get(
        &app,
        "/api/v2/organizations/acme-platform/workspaces/orders-api-prod",
    )
    .await;
    let data = &ws.body["data"];
    assert_eq!(data["id"], ORDERS_PROD);
    assert_eq!(data["attributes"]["locked"], true);
    assert_eq!(data["relationships"]["locked-by"]["data"]["type"], "runs");
    assert_eq!(
        data["relationships"]["project"]["data"]["id"],
        "prj-RGb4nwJ3c5kaUkvv"
    );

    let readme = get(
        &app,
        &format!("/api/v2/workspaces/{NET_PROD}?include=readme"),
    )
    .await;
    assert_eq!(readme.body["included"][0]["type"], "workspace-readme");
    let missing = get(&app, "/api/v2/organizations/acme-platform/workspaces/nope").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn workspace_create_update_and_safe_delete() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let created = send(
        &app,
        "POST",
        "/api/v2/organizations/acme-platform/workspaces",
        Some(json!({ "data": { "type": "workspaces", "attributes": { "name": "search-api-dev", "auto-apply": true, "source-name": "terraform-mcp-server" } } })),
        true,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = created.body["data"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        created.body["data"]["relationships"]["project"]["data"]["id"],
        "prj-Oc8bkFmEfGG81QGr"
    );

    let dup = send(
        &app,
        "POST",
        "/api/v2/organizations/acme-platform/workspaces",
        Some(
            json!({ "data": { "type": "workspaces", "attributes": { "name": "search-api-dev" } } }),
        ),
        true,
    )
    .await;
    assert_eq!(dup.status, StatusCode::UNPROCESSABLE_ENTITY);

    let updated = send(
        &app,
        "PATCH",
        "/api/v2/organizations/acme-platform/workspaces/search-api-dev",
        Some(json!({ "data": { "type": "workspaces", "attributes": { "name": "search-api-sandbox", "terraform-version": "1.10.0" } } })),
        true,
    )
    .await;
    assert_eq!(updated.status, StatusCode::OK);
    assert_eq!(
        state(&world)["workspaces"][&id]["name"],
        "search-api-sandbox"
    );

    let busy = send(
        &app,
        "POST",
        &format!("/api/v2/workspaces/{ORDERS_LEGACY}/actions/safe-delete"),
        None,
        true,
    )
    .await;
    assert_eq!(busy.status, StatusCode::CONFLICT);
    let ok = send(
        &app,
        "POST",
        &format!("/api/v2/workspaces/{id}/actions/safe-delete"),
        None,
        true,
    )
    .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert!(state(&world)["workspaces"].get(&id).is_none());
}

#[tokio::test]
async fn locking_rules() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let not_mine = send(
        &app,
        "POST",
        &format!("/api/v2/workspaces/{ORDERS_LEGACY}/actions/unlock"),
        None,
        true,
    )
    .await;
    assert_eq!(not_mine.status, StatusCode::CONFLICT);
    assert!(
        not_mine.body["errors"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("locked by User dmitri-volkov")
    );

    let forced = send(
        &app,
        "POST",
        &format!("/api/v2/workspaces/{ORDERS_PROD}/actions/force-unlock"),
        None,
        true,
    )
    .await;
    assert_eq!(forced.status, StatusCode::OK);
    assert_eq!(forced.body["data"]["attributes"]["locked"], false);
    assert_eq!(state(&world)["workspaces"][ORDERS_PROD]["locked"], false);

    let again = send(
        &app,
        "POST",
        &format!("/api/v2/workspaces/{ORDERS_PROD}/actions/force-unlock"),
        None,
        true,
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT);

    let lock = send(
        &app,
        "POST",
        &format!("/api/v2/workspaces/{ORDERS_PROD}/actions/lock"),
        Some(json!({ "reason": "maintenance" })),
        true,
    )
    .await;
    assert_eq!(lock.status, StatusCode::OK);
    assert_eq!(
        state(&world)["workspaces"][ORDERS_PROD]["locked-by"]["type"],
        "users"
    );
}

#[tokio::test]
async fn tags_and_tag_bindings() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let tags = get(
        &app,
        &format!("/api/v2/workspaces/{CHECKOUT_STG}/relationships/tags"),
    )
    .await;
    assert_eq!(names(&tags.body), ["checkout", "staging"]);
    let added = send(
        &app,
        "PATCH",
        &format!("/api/v2/workspaces/{CHECKOUT_STG}/tag-bindings"),
        Some(json!({ "data": [
            { "type": "tag-bindings", "attributes": { "key": "env", "value": "loadtest" } },
            { "type": "tag-bindings", "attributes": { "key": "cost-center", "value": "cc-42" } }
        ] })),
        true,
    )
    .await;
    assert_eq!(added.status, StatusCode::OK);
    assert_eq!(added.body["data"].as_array().unwrap().len(), 3);
    let bindings = &state(&world)["workspaces"][CHECKOUT_STG]["tag-bindings"];
    assert_eq!(bindings[1], json!({ "key": "env", "value": "loadtest" }));
}

#[tokio::test]
async fn workspace_variables_crud() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let vars = get(&app, &format!("/api/v2/workspaces/{CHECKOUT_STG}/vars")).await;
    assert_eq!(vars.body["data"].as_array().unwrap().len(), 6);
    let sensitive = vars.body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["attributes"]["key"] == "DD_API_KEY")
        .unwrap();
    assert_eq!(sensitive["attributes"]["value"], Value::Null);
    assert_eq!(
        sensitive["relationships"]["configurable"]["data"]["type"],
        "workspaces"
    );

    let dup = send(
        &app,
        "POST",
        &format!("/api/v2/workspaces/{CHECKOUT_STG}/vars"),
        Some(json!({ "data": { "type": "vars", "attributes": { "key": "desired_count", "value": "4", "category": "terraform" } } })),
        true,
    )
    .await;
    assert_eq!(dup.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        dup.body["errors"][0]["source"]["pointer"],
        "/data/attributes/key"
    );

    let created = send(
        &app,
        "POST",
        &format!("/api/v2/workspaces/{CHECKOUT_STG}/vars"),
        Some(json!({ "data": { "type": "vars", "attributes": { "key": "LOAD_TEST", "value": "true", "category": "env" } } })),
        true,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);

    let updated = send(
        &app,
        "PATCH",
        &format!("/api/v2/workspaces/{CHECKOUT_STG}/vars/{DESIRED_COUNT}"),
        Some(json!({ "data": { "type": "vars", "attributes": { "key": "desired_count", "value": "4" } } })),
        true,
    )
    .await;
    assert_eq!(updated.status, StatusCode::OK);
    assert_eq!(state(&world)["vars"][DESIRED_COUNT]["value"], "4");

    let wrong_ws = send(
        &app,
        "PATCH",
        &format!("/api/v2/workspaces/{NET_PROD}/vars/{DESIRED_COUNT}"),
        Some(json!({ "data": { "type": "vars", "attributes": { "value": "9" } } })),
        true,
    )
    .await;
    assert_eq!(wrong_ws.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn runs_list_newest_first_with_filters_and_included_workspace() {
    let app = service_app(world(), &Terraform);
    let runs = get(
        &app,
        "/api/v2/workspaces/ws-vJd7W10B1cG75Uxp/runs?include=workspace",
    )
    .await;
    let list = ids(&runs.body);
    assert_eq!(list[0], PAY_PROD_FAILED_RUN);
    assert_eq!(list.len(), 4);
    assert_eq!(runs.body["included"][0]["type"], "workspaces");

    let errored = get(
        &app,
        "/api/v2/workspaces/ws-vJd7W10B1cG75Uxp/runs?filter%5Bstatus%5D=errored",
    )
    .await;
    assert_eq!(errored.body["data"].as_array().unwrap().len(), 2);

    let org = get(
        &app,
        "/api/v2/organizations/acme-platform/runs?filter%5Bstatus%5D=planned&include=workspace",
    )
    .await;
    assert_eq!(ids(&org.body), [NET_PROD_RUN, NET_STG_RUN]);

    let run = get(&app, &format!("/api/v2/runs/{PAY_PROD_FAILED_RUN}")).await;
    assert_eq!(run.body["data"]["attributes"]["status"], "errored");
    assert_eq!(
        run.body["data"]["relationships"]["apply"]["data"]["id"],
        PAY_PROD_FAILED_APPLY
    );
}

#[tokio::test]
async fn plans_applies_logs_and_json_output() {
    let app = service_app(world(), &Terraform);
    let plan = get(&app, &format!("/api/v2/plans/{PAY_PROD_FAILED_PLAN}")).await;
    let attrs = &plan.body["data"]["attributes"];
    assert_eq!(attrs["resource-additions"], 1);
    assert_eq!(attrs["resource-changes"], 1);
    let log_url = attrs["log-read-url"].as_str().unwrap();
    assert!(log_url.starts_with("http://world:8109/archivist/"));
    let path = log_url.trim_start_matches("http://world:8109");
    let log = send(
        &app,
        "GET",
        &format!("{path}?limit=100000&offset=0"),
        None,
        false,
    )
    .await;
    assert_eq!(log.raw[0], 2);
    assert_eq!(*log.raw.last().unwrap(), 3);
    let text = String::from_utf8_lossy(&log.raw);
    assert!(text.contains("Plan: 1 to add, 1 to change, 0 to destroy."));
    let tail = send(
        &app,
        "GET",
        &format!("{path}?limit=10&offset=100000"),
        None,
        false,
    )
    .await;
    assert!(tail.raw.is_empty());

    let apply = get(&app, &format!("/api/v2/applies/{PAY_PROD_FAILED_APPLY}")).await;
    assert_eq!(apply.body["data"]["attributes"]["status"], "errored");
    let apply_log_path = apply.body["data"]["attributes"]["log-read-url"]
        .as_str()
        .unwrap()
        .trim_start_matches("http://world:8109")
        .to_owned();
    let apply_log = send(
        &app,
        "GET",
        &format!("{apply_log_path}?limit=100000&offset=0"),
        None,
        false,
    )
    .await;
    assert!(String::from_utf8_lossy(&apply_log.raw).contains("InvalidParameterCombination"));

    let json_out = get(
        &app,
        &format!("/api/v2/plans/{PAY_PROD_FAILED_PLAN}/json-output"),
    )
    .await;
    assert_eq!(json_out.status, StatusCode::TEMPORARY_REDIRECT);
    let location = json_out.headers["location"]
        .to_str()
        .unwrap()
        .trim_start_matches("http://world:8109")
        .to_owned();
    let plan_json = get(&app, &location).await;
    assert_eq!(
        plan_json.body["resource_changes"][1]["address"],
        "aws_db_instance.payments"
    );
    assert_eq!(
        plan_json.body["resource_changes"][1]["change"]["actions"],
        json!(["update"])
    );
}

#[tokio::test]
async fn create_run_plans_immediately_and_records_inputs() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let body = json!({ "data": { "type": "runs", "attributes": { "message": "Scale up", "auto-apply": false },
        "relationships": { "workspace": { "data": { "type": "workspaces", "id": CHECKOUT_STG } } } } });
    let created = send(&app, "POST", "/api/v2/runs", Some(body), true).await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.body["data"]["attributes"]["status"], "planned");
    assert_eq!(
        created.body["data"]["attributes"]["actions"]["is-confirmable"],
        true
    );
    let run_id = created.body["data"]["id"].as_str().unwrap().to_owned();
    let snap = state(&world);
    let ws = &snap["workspaces"][CHECKOUT_STG];
    assert_eq!(ws["current-run"], run_id.as_str());
    assert_eq!(ws["latest-run-summary"]["status"], "planned");
    assert_eq!(
        ws["latest-run-summary"]["input-variables"]["desired_count"],
        "2"
    );
    assert_eq!(
        ws["latest-run-summary"]["input-variables"]["DD_API_KEY"],
        "(sensitive value)"
    );
    let plan_id = snap["runs"][&run_id]["plan"].as_str().unwrap();
    assert_eq!(snap["plans"][plan_id]["resource-changes"], 2);

    let plan_only = json!({ "data": { "type": "runs", "attributes": { "plan-only": true },
        "relationships": { "workspace": { "data": { "type": "workspaces", "id": CHECKOUT_STG } } } } });
    let speculative = send(&app, "POST", "/api/v2/runs", Some(plan_only), true).await;
    assert_eq!(
        speculative.body["data"]["attributes"]["status"],
        "planned_and_finished"
    );
    assert_eq!(
        state(&world)["workspaces"][CHECKOUT_STG]["current-run"],
        run_id.as_str()
    );

    let missing = json!({ "data": { "type": "runs", "relationships": { "workspace": { "data": { "type": "workspaces", "id": "ws-nope" } } } } });
    assert_eq!(
        send(&app, "POST", "/api/v2/runs", Some(missing), true)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn run_actions_apply_discard_cancel() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let apply = send(
        &app,
        "POST",
        &format!("/api/v2/runs/{NET_PROD_RUN}/actions/apply"),
        Some(json!({ "comment": "Approved per CHG-2291" })),
        true,
    )
    .await;
    assert_eq!(apply.status, StatusCode::ACCEPTED);
    let snap = state(&world);
    assert_eq!(snap["runs"][NET_PROD_RUN]["status"], "applied");
    assert_eq!(
        snap["runs"][NET_PROD_RUN]["confirm-comment"],
        "Approved per CHG-2291"
    );
    assert_eq!(snap["workspaces"][NET_PROD]["locked"], false);
    assert_eq!(snap["workspaces"][NET_PROD]["resource-count"], 60);
    let sv = snap["workspaces"][NET_PROD]["current-state-version"]
        .as_str()
        .unwrap();
    assert_eq!(snap["state_versions"][sv]["serial"], 97);

    let again = send(
        &app,
        "POST",
        &format!("/api/v2/runs/{NET_PROD_RUN}/actions/apply"),
        None,
        true,
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.body["errors"][0]["title"], "transition not allowed");

    let discard = send(
        &app,
        "POST",
        &format!("/api/v2/runs/{NET_STG_RUN}/actions/discard"),
        None,
        true,
    )
    .await;
    assert_eq!(discard.status, StatusCode::ACCEPTED);
    assert_eq!(state(&world)["runs"][NET_STG_RUN]["status"], "discarded");

    let cancel_done = send(
        &app,
        "POST",
        &format!("/api/v2/runs/{PAY_PROD_FAILED_RUN}/actions/cancel"),
        None,
        true,
    )
    .await;
    assert_eq!(cancel_done.status, StatusCode::CONFLICT);
    let stuck = "run-jCcDubzVP5ob0NM7";
    let cancel = send(
        &app,
        "POST",
        &format!("/api/v2/runs/{stuck}/actions/cancel"),
        None,
        true,
    )
    .await;
    assert_eq!(cancel.status, StatusCode::ACCEPTED);
    assert_eq!(state(&world)["workspaces"][ORDERS_PROD]["locked"], false);
}

#[tokio::test]
async fn comments_and_policy_checks() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let comments = get(&app, &format!("/api/v2/runs/{NET_PROD_RUN}/comments")).await;
    assert!(
        comments.body["data"][0]["attributes"]["body"]
            .as_str()
            .unwrap()
            .contains("CHG-2291")
    );
    let created = send(
        &app,
        "POST",
        &format!("/api/v2/runs/{NET_PROD_RUN}/comments"),
        Some(json!({ "data": { "type": "comments", "attributes": { "body": "LGTM" } } })),
        true,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let checks = get(&app, "/api/v2/runs/run-1rQ3sGqHrZ0U4fJx/policy-checks").await;
    assert_eq!(checks.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn plan_exports_produce_a_gzip_bundle() {
    let app = service_app(world(), &Terraform);
    let created = send(
        &app,
        "POST",
        "/api/v2/plan-exports",
        Some(json!({ "data": { "type": "plan-exports", "attributes": { "data-type": "sentinel-mock-bundle-v0" },
            "relationships": { "plan": { "data": { "type": "plans", "id": PAY_PROD_FAILED_PLAN } } } } })),
        true,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.body["data"]["attributes"]["status"], "queued");
    let id = created.body["data"]["id"].as_str().unwrap().to_owned();
    let read = get(&app, &format!("/api/v2/plan-exports/{id}")).await;
    assert_eq!(read.body["data"]["attributes"]["status"], "finished");
    let download = get(&app, &format!("/api/v2/plan-exports/{id}/download")).await;
    assert_eq!(download.status, StatusCode::FOUND);
    let location = download.headers["location"]
        .to_str()
        .unwrap()
        .trim_start_matches("http://world:8109")
        .to_owned();
    let bundle = get(&app, &location).await;
    assert_eq!(&bundle.raw[..2], &[0x1f, 0x8b]);
}

#[tokio::test]
async fn state_versions() {
    let app = service_app(world(), &Terraform);
    let list = get(
        &app,
        "/api/v2/state-versions?filter%5Borganization%5D%5Bname%5D=acme-platform&filter%5Bworkspace%5D%5Bname%5D=network-core-prod",
    )
    .await;
    assert_eq!(list.body["data"][0]["attributes"]["serial"], 96);
    let current = get(
        &app,
        &format!("/api/v2/workspaces/{NET_PROD}/current-state-version?include=outputs"),
    )
    .await;
    assert_eq!(
        current.body["data"]["relationships"]["outputs"]["data"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(current.body["included"][0]["attributes"]["name"], "vpc_id");
    let missing = get(
        &app,
        "/api/v2/state-versions?filter%5Borganization%5D%5Bname%5D=acme-platform&filter%5Bworkspace%5D%5Bname%5D=nope",
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn variable_sets_and_policy_sets() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let sets = get(
        &app,
        "/api/v2/organizations/acme-platform/varsets?include=&q=oidc",
    )
    .await;
    assert_eq!(names(&sets.body), ["aws-prod-oidc", "aws-staging-oidc"]);
    let created = send(
        &app,
        "POST",
        "/api/v2/organizations/acme-platform/varsets",
        Some(json!({ "data": { "type": "varsets", "attributes": { "name": "loadtest", "global": false } } })),
        true,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = created.body["data"]["id"].as_str().unwrap().to_owned();
    let var = send(
        &app,
        "POST",
        &format!("/api/v2/varsets/{id}/relationships/vars"),
        Some(json!({ "data": { "type": "vars", "attributes": { "key": "rps", "value": "500", "category": "terraform" } } })),
        true,
    )
    .await;
    assert_eq!(var.status, StatusCode::CREATED);
    let attach = send(
        &app,
        "POST",
        &format!("/api/v2/varsets/{id}/relationships/workspaces"),
        Some(json!({ "data": [{ "type": "workspaces", "id": CHECKOUT_STG }] })),
        true,
    )
    .await;
    assert_eq!(attach.status, StatusCode::NO_CONTENT);
    assert_eq!(
        state(&world)["varsets"][&id]["workspaces"],
        json!([CHECKOUT_STG])
    );

    let policy_sets = get(
        &app,
        "/api/v2/organizations/acme-platform/policy-sets?include=workspaces&page%5Bsize%5D=100",
    )
    .await;
    assert_eq!(
        names(&policy_sets.body),
        ["cis-aws-baseline", "payments-pci", "required-tags-opa"]
    );
    assert_eq!(
        policy_sets.body["meta"]["pagination"]["next-page"],
        Value::Null
    );
    let global = "polset-";
    let cis = ids(&policy_sets.body)[0].clone();
    assert!(cis.starts_with(global));
    let refused = send(
        &app,
        "POST",
        &format!("/api/v2/policy-sets/{cis}/relationships/workspaces"),
        Some(json!({ "data": [{ "type": "workspaces", "id": CHECKOUT_STG }] })),
        true,
    )
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn teams_members_and_access() {
    let world = world();
    let app = service_app(world.clone(), &Terraform);
    let teams = get(&app, "/api/v2/organizations/acme-platform/teams?q=pay").await;
    assert_eq!(names(&teams.body), ["payments-dev"]);
    let team = ids(&teams.body)[0].clone();
    let add = send(
        &app,
        "POST",
        &format!("/api/v2/teams/{team}/relationships/users"),
        Some(json!({ "data": [{ "type": "users", "id": "priya-nair" }] })),
        true,
    )
    .await;
    assert_eq!(add.status, StatusCode::NO_CONTENT);
    assert_eq!(
        state(&world)["teams"][&team]["users"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let stranger = send(
        &app,
        "POST",
        &format!("/api/v2/teams/{team}/relationships/users"),
        Some(json!({ "data": [{ "type": "users", "id": "nobody" }] })),
        true,
    )
    .await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND);

    let grant = send(
        &app,
        "POST",
        "/api/v2/team-workspaces",
        Some(json!({ "data": { "type": "team-workspaces", "attributes": { "access": "plan" },
            "relationships": { "team": { "data": { "type": "teams", "id": team } }, "workspace": { "data": { "type": "workspaces", "id": CHECKOUT_STG } } } } })),
        true,
    )
    .await;
    assert_eq!(grant.status, StatusCode::OK);
    assert_eq!(grant.body["data"]["attributes"]["runs"], "plan");
    assert_eq!(
        grant.body["data"]["relationships"]["workspace"]["data"]["id"],
        CHECKOUT_STG
    );

    let created = send(
        &app,
        "POST",
        "/api/v2/organizations/acme-platform/teams",
        Some(json!({ "data": { "type": "teams", "attributes": { "name": "sre", "visibility": "organization" } } })),
        true,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let sre = created.body["data"]["id"].as_str().unwrap().to_owned();
    let project_grant = send(
        &app,
        "POST",
        "/api/v2/team-projects",
        Some(json!({ "data": { "type": "team-projects", "attributes": { "access": "maintain" },
            "relationships": { "team": { "data": { "type": "teams", "id": sre } }, "project": { "data": { "type": "projects", "id": "prj-RGb4nwJ3c5kaUkvv" } } } } })),
        true,
    )
    .await;
    assert_eq!(project_grant.status, StatusCode::CREATED);
    let deleted = send(&app, "DELETE", &format!("/api/v2/teams/{sre}"), None, true).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(state(&world)["team_projects"].as_object().unwrap().len(), 3);
}

#[tokio::test]
async fn stacks_and_private_registry() {
    let app = service_app(world(), &Terraform);
    let stacks = get(
        &app,
        "/api/v2/organizations/acme-platform/stacks?search%5Bname%5D=edge",
    )
    .await;
    assert_eq!(names(&stacks.body), ["edge-cdn"]);
    let stack = get(&app, &format!("/api/v2/stacks/{}", ids(&stacks.body)[0])).await;
    assert_eq!(stack.body["data"]["type"], "stacks");

    let modules = get(
        &app,
        "/api/v2/organizations/acme-platform/registry-modules?include=no-code-modules&q=ecs",
    )
    .await;
    assert_eq!(names(&modules.body), ["ecs-service"]);
    assert_eq!(modules.body["data"][0]["attributes"]["no-code"], true);
    assert_eq!(modules.body["included"][0]["type"], "no-code-modules");

    let module = get(&app, "/api/v2/organizations/acme-platform/registry-modules/private/acme-platform/ecs-service/aws").await;
    assert_eq!(module.status, StatusCode::OK);
    let details = get(
        &app,
        "/api/registry/v1/modules/acme-platform/ecs-service/aws",
    )
    .await;
    assert_eq!(details.body["version"], "3.2.0");
    assert_eq!(details.body["root"]["inputs"][0]["name"], "name");
    let metadata = get(
        &app,
        "/api/registry/private/v2/modules/acme-platform/ecs-service/aws/metadata/3.2.0?organization_name=acme-platform",
    )
    .await;
    assert_eq!(
        metadata.body["data"]["attributes"]["input-variables"]
            .as_array()
            .unwrap()
            .len(),
        5
    );

    let providers = get(&app, "/api/v2/organizations/acme-platform/registry-providers?filter%5Bregistry_name%5D=private&include=registry-provider-versions").await;
    assert_eq!(names(&providers.body), ["internaldns"]);
    let provider = get(&app, "/api/v2/organizations/acme-platform/registry-providers/private/acme-platform/internaldns?include=registry-provider-versions").await;
    assert_eq!(
        provider.body["included"][0]["attributes"]["version"],
        "1.3.1"
    );
    let none = get(
        &app,
        "/api/v2/organizations/acme-platform/registry-providers/private/acme-platform/nope",
    )
    .await;
    assert_eq!(none.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn public_registry_providers_and_docs() {
    let app = service_app(empty_world(), &Terraform);
    let latest = send(&app, "GET", "/v1/providers/hashicorp/aws", None, false).await;
    assert_eq!(latest.body["version"], "5.82.2");
    let missing = send(&app, "GET", "/v1/providers/hashicorp/nope", None, false).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.body, json!({ "errors": ["Not Found"] }));

    let version = send(
        &app,
        "GET",
        "/v1/providers/hashicorp/aws/5.81.0",
        None,
        false,
    )
    .await;
    let docs = version.body["docs"].as_array().unwrap();
    assert!(
        docs.iter()
            .any(|d| d["slug"] == "nat_gateway" && d["category"] == "resources")
    );

    let v2 = send(
        &app,
        "GET",
        "/v2/providers/hashicorp/aws?include=provider-versions",
        None,
        false,
    )
    .await;
    assert_eq!(v2.body["included"].as_array().unwrap().len(), 3);
    let version_id = v2.body["included"][2]["id"].as_str().unwrap().to_owned();

    let guides = send(
        &app,
        "GET",
        &format!("/v2/provider-docs?filter[provider-version]={version_id}&filter[category]=guides&filter[language]=hcl&page[number]=1"),
        None,
        false,
    )
    .await;
    assert_eq!(guides.body["data"].as_array().unwrap().len(), 1);
    let page2 = send(
        &app,
        "GET",
        &format!("/v2/provider-docs?filter[provider-version]={version_id}&filter[category]=guides&filter[language]=hcl&page[number]=2"),
        None,
        false,
    )
    .await;
    assert!(page2.body["data"].as_array().unwrap().is_empty());
    let doc_id = guides.body["data"][0]["id"].as_str().unwrap().to_owned();
    let doc = send(
        &app,
        "GET",
        &format!("/v2/provider-docs/{doc_id}"),
        None,
        false,
    )
    .await;
    assert!(
        doc.body["data"]["attributes"]["content"]
            .as_str()
            .unwrap()
            .contains("Upgrade Guide")
    );
}

#[tokio::test]
async fn public_registry_modules_and_policies() {
    let app = service_app(empty_world(), &Terraform);
    let search = send(
        &app,
        "GET",
        "/v1/modules/search?q='vpc'&offset=0",
        None,
        false,
    )
    .await;
    assert_eq!(
        search.body["modules"][0]["id"],
        "terraform-aws-modules/vpc/aws/5.16.0"
    );
    let details = send(
        &app,
        "GET",
        "/v1/modules/terraform-aws-modules/vpc/aws/5.15.0?offset=0",
        None,
        false,
    )
    .await;
    assert_eq!(details.body["version"], "5.15.0");
    assert!(
        !details.body["root"]["inputs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let unknown = send(
        &app,
        "GET",
        "/v1/modules/terraform-aws-modules/vpc/aws/0.0.1",
        None,
        false,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let latest = send(
        &app,
        "GET",
        "/v1/modules/terraform-aws-modules/eks/aws",
        None,
        false,
    )
    .await;
    assert_eq!(latest.body["version"], "20.31.6");

    let policies = send(
        &app,
        "GET",
        "/v2/policies?page%5Bsize%5D=100&include=latest-version",
        None,
        false,
    )
    .await;
    let first = &policies.body["data"][0];
    let related = first["relationships"]["latest-version"]["links"]["related"]
        .as_str()
        .unwrap();
    assert_eq!(
        related,
        "/v2/policies/hashicorp/CIS-Policy-Set-for-AWS-Terraform/1.0.1"
    );
    let details = send(
        &app,
        "GET",
        &format!("{related}?include=policies%2Cpolicy-modules%2Cpolicy-library"),
        None,
        false,
    )
    .await;
    let included = details.body["included"].as_array().unwrap();
    assert_eq!(
        included.iter().filter(|i| i["type"] == "policies").count(),
        4
    );
    assert!(
        details.body["data"]["attributes"]["readme"]
            .as_str()
            .unwrap()
            .starts_with("# Pre-written")
    );
}
