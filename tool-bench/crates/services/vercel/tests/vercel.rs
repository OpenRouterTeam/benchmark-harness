use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_vercel::Vercel;
use serde_json::{Value, json};
use tower::ServiceExt;

const STOREFRONT: &str = "prj_3dw2VoUa166qmaLV3P6dm828isAX";
const CHECKOUT_NEXT: &str = "prj_9NauQLxYvOeIyWZWq6mqvavv9xuH";
const LIVE_STOREFRONT: &str = "dpl_8o704GjcwumkLAaxPwjoxb4V6QDF";
const ACME: &str = "team_9vXcR2kLmQ4tZ8wN3pHf6sJd";

fn world(slug: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/code/vercel")
        .join(slug);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn app(world: &SharedWorld) -> Router {
    service_app(world.clone(), &Vercel)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer tb")
        .header("content-type", "application/json")
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn tool(app: &Router, name: &str, arguments: Value) -> Value {
    let (status, result) = call(
        app,
        "POST",
        "/_mcp/tools/call",
        Some(json!({ "name": name, "arguments": arguments })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    result
}

fn texts(result: &Value) -> Vec<String> {
    result["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["text"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn state(world: &SharedWorld) -> Value {
    world.lock().unwrap().snapshot()["services"]["vercel"].clone()
}

// ───────────── REST ─────────────

#[tokio::test]
async fn rest_requires_a_bearer_token() {
    let w = world("storefront-production-status");
    let req = Request::get("/v9/projects").body(Body::empty()).unwrap();
    let resp = app(&w).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lists_deployments_newest_first_with_filters_and_pagination() {
    let w = world("storefront-production-status");
    let app = app(&w);
    let (status, body) = call(
        &app,
        "GET",
        "/v6/deployments?projectId=storefront&limit=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let deps = body["deployments"].as_array().unwrap();
    assert_eq!(deps.len(), 2);
    assert_eq!(deps[0]["meta"]["githubCommitRef"], "feat/gift-cards");
    assert_eq!(deps[0]["state"], "ERROR");
    let next = body["pagination"]["next"].as_i64().unwrap();
    let (_, page2) = call(
        &app,
        "GET",
        &format!("/v6/deployments?projectId=storefront&limit=2&until={next}"),
        None,
    )
    .await;
    assert_eq!(
        page2["deployments"][0]["meta"]["githubCommitRef"],
        "fix/cart-badge"
    );

    let (_, prod) = call(
        &app,
        "GET",
        "/v6/deployments?projectId=storefront&target=production&state=READY",
        None,
    )
    .await;
    let prod = prod["deployments"].as_array().unwrap();
    assert_eq!(prod.len(), 2);
    assert_eq!(prod[0]["uid"], LIVE_STOREFRONT);
    assert_eq!(prod[0]["readySubstate"], "PROMOTED");
    assert_eq!(prod[1]["isRollbackCandidate"], true);

    let (_, none) = call(&app, "GET", "/v6/deployments?projectId=nope", None).await;
    assert_eq!(none["deployments"], json!([]));
}

#[tokio::test]
async fn gets_deployment_by_id_or_url_and_404s() {
    let w = world("storefront-production-status");
    let app = app(&w);
    let (status, d) = call(
        &app,
        "GET",
        "/v13/deployments/dpl_1KT70nyZgoWTlCSRmxLe0svxe4Xx",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(d["errorMessage"].as_str().unwrap().contains("250 MB"));
    let (status, by_alias) = call(&app, "GET", "/v13/deployments/acme-shop.com", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(by_alias["id"], LIVE_STOREFRONT);
    let (status, err) = call(&app, "GET", "/v13/deployments/dpl_missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not_found");
    let (status, _) = call(
        &app,
        "GET",
        "/v13/deployments/dpl_missing?teamId=team_nope",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deployment_files_is_a_bare_array() {
    let w = world("storefront-production-status");
    let app = app(&w);
    let (status, files) = call(
        &app,
        "GET",
        &format!("/v6/deployments/{LIVE_STOREFRONT}/files"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(files[0]["name"], "src");
    assert_eq!(files[0]["type"], "directory");
}

#[tokio::test]
async fn projects_list_search_get_and_team_scope() {
    let w = world("storefront-production-status");
    let app = app(&w);
    let (_, all) = call(&app, "GET", "/v9/projects", None).await;
    assert_eq!(all["projects"].as_array().unwrap().len(), 6);
    assert!(all["projects"][0].get("env").is_none());
    let (_, search) = call(&app, "GET", "/v9/projects?search=checkout", None).await;
    let names: Vec<&str> = search["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["checkout-api-next", "checkout-api"]);
    let (_, labs) = call(
        &app,
        "GET",
        "/v9/projects?teamId=team_2bHq7YtR5mWx9cKp4LnZ8vFe",
        None,
    )
    .await;
    assert_eq!(labs["projects"].as_array().unwrap().len(), 1);
    let (_, paged) = call(&app, "GET", "/v9/projects?limit=4", None).await;
    assert_eq!(paged["pagination"]["count"], 4);
    assert!(paged["pagination"]["next"].is_i64());

    let (status, p) = call(&app, "GET", "/v9/projects/storefront", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["targets"]["production"]["id"], LIVE_STOREFRONT);
    assert_eq!(p["env"].as_array().unwrap().len(), 6);
    let (status, err) = call(
        &app,
        "GET",
        "/v9/projects/storefront?teamId=team_2bHq7YtR5mWx9cKp4LnZ8vFe",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        err,
        json!({"error": {"code": "not_found", "message": "Project not found"}})
    );
}

#[tokio::test]
async fn create_and_delete_project() {
    let w = world("storefront-production-status");
    let app = app(&w);
    let (status, p) = call(
        &app,
        "POST",
        &format!("/v11/projects?teamId={ACME}"),
        Some(json!({"name": "status-page", "framework": "astro"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = p["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("prj_"));
    assert_eq!(state(&w)["projects"][&id]["accountId"], ACME);
    let (status, err) = call(
        &app,
        "POST",
        "/v11/projects",
        Some(json!({"name": "status-page"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"]["code"], "conflict");
    let (status, _) = call(
        &app,
        "POST",
        "/v11/projects",
        Some(json!({"name": "Bad Name"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = call(&app, "DELETE", "/v9/projects/status-page", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(state(&w)["projects"].get(&id).is_none());
    let (status, _) = call(&app, "DELETE", "/v9/projects/status-page", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn env_list_masks_values_create_conflicts_and_delete() {
    let w = world("rotate-checkout-stripe-key");
    let app = app(&w);
    let (_, list) = call(&app, "GET", "/v10/projects/checkout-api-next/env", None).await;
    let envs = list["envs"].as_array().unwrap();
    assert!(
        envs[0].get("value").is_none(),
        "sensitive values are never returned"
    );
    assert_ne!(envs[1]["value"], "sk_test_51NvCheckoutNextTst2");
    assert_eq!(envs[4]["value"], "info");

    let body = json!([{"key": "STRIPE_SECRET_KEY", "value": "x", "type": "sensitive", "target": ["production"]}]);
    let (status, r) = call(
        &app,
        "POST",
        "/v10/projects/checkout-api-next/env",
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(r["created"], json!([]));
    assert_eq!(r["failed"][0]["error"]["code"], "ENV_CONFLICT");

    let (status, _) = call(
        &app,
        "POST",
        "/v10/projects/checkout-api-next/env",
        Some(json!({"key": "1BAD", "value": "x", "type": "plain", "target": ["preview"]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, removed) = call(
        &app,
        "DELETE",
        "/v9/projects/checkout-api-next/env/rN1Dlb7NzGIlkNJD",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed["key"], "STRIPE_SECRET_KEY");
    let (status, err) = call(
        &app,
        "DELETE",
        "/v9/projects/checkout-api-next/env/rN1Dlb7NzGIlkNJD",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not_found");

    let (status, r) = call(
        &app,
        "POST",
        "/v10/projects/checkout-api-next/env",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(r["created"][0]["key"], "STRIPE_SECRET_KEY");
    let s = state(&w);
    assert_eq!(
        s["env_by_target"][CHECKOUT_NEXT]["production"]["STRIPE_SECRET_KEY"],
        "x"
    );
    assert_eq!(
        s["env_by_target"][CHECKOUT_NEXT]["preview"]["STRIPE_SECRET_KEY"],
        "sk_test_51NvCheckoutNextTst2"
    );
}

#[tokio::test]
async fn domains_and_custom_environments() {
    let w = world("rotate-checkout-stripe-key");
    let app = app(&w);
    let (status, d) = call(
        &app,
        "GET",
        "/v9/projects/checkout-api-next/domains/api.acme-shop.com",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(d["verified"], true);
    let (status, err) = call(
        &app,
        "GET",
        "/v9/projects/checkout-api/domains/api.acme-shop.com",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not_found");
    let (_, list) = call(&app, "GET", "/v9/projects/storefront/domains", None).await;
    assert_eq!(list["domains"].as_array().unwrap().len(), 3);

    let (status, env) = call(
        &app,
        "POST",
        "/v9/projects/storefront/custom-environments",
        Some(json!({"slug": "qa", "branchMatcher": {"type": "startsWith", "pattern": "qa/"}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(env["id"].as_str().unwrap().starts_with("env_"));
    assert!(
        state(&w)["custom_environments"]
            .get(format!("{STOREFRONT}/qa"))
            .is_some()
    );
    let (status, _) = call(
        &app,
        "POST",
        "/v9/projects/storefront/custom-environments",
        Some(json!({"slug": "qa"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, list) = call(
        &app,
        "GET",
        "/v9/projects/storefront/custom-environments",
        None,
    )
    .await;
    assert_eq!(list["environments"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn teams_list_get_and_create() {
    let w = world("storefront-production-status");
    let app = app(&w);
    let (_, teams) = call(&app, "GET", "/v2/teams", None).await;
    assert_eq!(teams["teams"][0]["slug"], "acme-labs");
    let (_, one) = call(&app, "GET", "/v2/teams?limit=1", None).await;
    assert_eq!(one["teams"].as_array().unwrap().len(), 1);
    assert!(one["pagination"]["next"].is_i64());
    let (status, t) = call(&app, "GET", "/v2/teams/acme-shop", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(t["id"], ACME);
    let (status, created) = call(
        &app,
        "POST",
        "/v1/teams",
        Some(json!({"slug": "acme-data", "name": "Acme Data"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(created["billing"].is_object());
    let (status, _) = call(
        &app,
        "POST",
        "/v1/teams",
        Some(json!({"slug": "acme-data"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(&app, "GET", "/v2/user", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn unknown_route_is_vercel_404() {
    let w = world("storefront-production-status");
    let (status, err) = call(&app(&w), "GET", "/v1/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not_found");
}

// ───────────── tools ─────────────

#[tokio::test]
async fn tool_list_and_get_deployment_format_like_the_server() {
    let w = world("storefront-production-status");
    let app = app(&w);
    let r = tool(
        &app,
        "vercel-list-all-deployments",
        json!({"projectId": "storefront", "target": "production"}),
    )
    .await;
    let list: Value = serde_json::from_str(&texts(&r)[0]).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 3);
    let r = tool(
        &app,
        "vercel-get-deployment",
        json!({"idOrUrl": LIVE_STOREFRONT}),
    )
    .await;
    let d: Value = serde_json::from_str(&texts(&r)[0]).unwrap();
    assert_eq!(
        d["meta"]["githubCommitSha"],
        "c3a9f12e8b47d05a6e21f9b3c7d84e50a1b6f2d9"
    );
    let r = tool(
        &app,
        "vercel-get-deployment",
        json!({"idOrUrl": "dpl_nope"}),
    )
    .await;
    assert_eq!(
        r,
        json!({"content": [{"type": "text", "text": "Failed to retrieve deployment"}]})
    );
    let r = tool(&app, "vercel-get-deployment", json!({})).await;
    assert!(texts(&r)[0].starts_with("Error: ZodError: [\n  {\n    \"code\": \"invalid_type\""));
}

#[tokio::test]
async fn tool_list_deployment_files_fails_like_the_server() {
    let w = world("storefront-production-status");
    let r = tool(
        &app(&w),
        "vercel-list-deployment-files",
        json!({"id": LIVE_STOREFRONT}),
    )
    .await;
    assert_eq!(
        r,
        json!({"content": [{"type": "text", "text": "Failed to retrieve deployment files or no files found", "isError": true}]})
    );
}

#[tokio::test]
async fn tool_projects_find_domain_and_environments() {
    let w = world("rotate-checkout-stripe-key");
    let app = app(&w);
    let r = tool(&app, "vercel-list-projects", json!({"search": "checkout"})).await;
    assert_eq!(texts(&r)[0], "Found 2 projects");
    let r = tool(&app, "vercel-list-projects", json!({"limit": 0})).await;
    assert_eq!(r["content"][0]["isError"], true);
    assert!(texts(&r)[0].contains("Number must be greater than 0"));
    let r = tool(
        &app,
        "vercel-find-project",
        json!({"idOrName": "checkout-api-next"}),
    )
    .await;
    assert_eq!(
        texts(&r)[0],
        format!("Found project: checkout-api-next ({CHECKOUT_NEXT})")
    );
    let r = tool(&app, "vercel-find-project", json!({"idOrName": "nope"})).await;
    assert_eq!(
        r,
        json!({"content": [{"type": "text", "text": "Project not found", "isError": true}]})
    );
    let r = tool(
        &app,
        "vercel-get-project-domain",
        json!({"idOrName": "checkout-api-next", "domain": "api.acme-shop.com"}),
    )
    .await;
    assert_eq!(
        texts(&r)[0],
        "Found domain: api.acme-shop.com (verified: true)"
    );
    let r = tool(
        &app,
        "vercel-get-project-domain",
        json!({"idOrName": "checkout-api", "domain": "api.acme-shop.com"}),
    )
    .await;
    assert_eq!(texts(&r)[0], "Domain not found");
    let r = tool(
        &app,
        "vercel-get-environments",
        json!({"idOrName": "checkout-api-next"}),
    )
    .await;
    assert_eq!(
        texts(&r)[0],
        "Retrieving 5 environment variables for project: checkout-api-next"
    );
    let r = tool(&app, "vercel-get-environments", json!({})).await;
    assert_eq!(
        texts(&r)[0],
        "Invalid request: idOrName parameter must be a non-empty string, received: undefined"
    );
    let r = tool(&app, "vercel-get-environments", json!({"idOrName": "nope"})).await;
    assert_eq!(
        texts(&r)[0],
        "Failed to retrieve environment variables for project: nope"
    );
}

#[tokio::test]
async fn tool_env_rotation_updates_state() {
    let w = world("rotate-checkout-stripe-key");
    let app = app(&w);
    let create = json!({"projectId": "checkout-api-next", "environmentVariables": [
        {"key": "STRIPE_SECRET_KEY", "value": "sk_live_new", "target": ["production"], "type": "sensitive"}]});
    let r = tool(&app, "vercel-create-environment-variables", create.clone()).await;
    assert_eq!(texts(&r)[0], "Successfully created 0 environment variables");
    assert!(texts(&r)[1].contains("ENV_CONFLICT"));
    let r = tool(
        &app,
        "vercel-remove-environment-variable",
        json!({"idOrName": "checkout-api-next", "id": "rN1Dlb7NzGIlkNJD"}),
    )
    .await;
    assert_eq!(
        texts(&r)[0],
        "Environment variable rN1Dlb7NzGIlkNJD removed successfully from project checkout-api-next"
    );
    let r = tool(&app, "vercel-create-environment-variables", create).await;
    assert_eq!(texts(&r)[0], "Successfully created 1 environment variables");
    assert_eq!(
        state(&w)["env_by_target"][CHECKOUT_NEXT]["production"]["STRIPE_SECRET_KEY"],
        "sk_live_new"
    );

    let r = tool(&app, "vercel-create-environment-variables", json!({"projectId": "x", "environmentVariables": [{"key": "A", "value": "1", "target": ["prod"], "type": "plain"}]})).await;
    assert_eq!(r["content"][0]["isError"], true);
    assert!(texts(&r)[0].contains(
        "Invalid enum value. Expected 'production' | 'preview' | 'development', received 'prod'"
    ));
    // A missing project: the server prints its null branch.
    let r = tool(
        &app,
        "vercel-remove-environment-variable",
        json!({"idOrName": "nope", "id": "x"}),
    )
    .await;
    assert_eq!(texts(&r)[1], "null");
}

#[tokio::test]
async fn tool_redeploy_moves_production() {
    let w = world("rollback-storefront-production");
    let app = app(&w);
    let r = tool(
        &app,
        "vercel-create-deployment",
        json!({"name": "storefront", "deploymentId": LIVE_STOREFRONT}),
    )
    .await;
    let t = texts(&r);
    assert!(t[0].starts_with("Deployment created successfully: storefront-"));
    let d: Value = serde_json::from_str(&t[1]).unwrap();
    assert_eq!(d["readyState"], "QUEUED");
    let s = state(&w);
    let prod = &s["projects"][STOREFRONT]["targets"]["production"];
    assert_eq!(
        prod["meta"]["githubCommitSha"],
        "c3a9f12e8b47d05a6e21f9b3c7d84e50a1b6f2d9"
    );
    assert!(
        prod["alias"]
            .as_array()
            .unwrap()
            .contains(&json!("acme-shop.com"))
    );
    assert_eq!(
        s["deployments"]["dpl_7wC1vKiBnuwDgHLXzdLRklqAYci5"]["alias"],
        json!([])
    );

    // Redeploying a failed build fails again and production stays put.
    tool(
        &app,
        "vercel-create-deployment",
        json!({"deploymentId": "dpl_PHkhNPm61EU1aF79kakZb9UhQNNQ"}),
    )
    .await;
    let s = state(&w);
    assert_eq!(
        s["projects"][STOREFRONT]["targets"]["production"]["meta"]["githubCommitSha"],
        "c3a9f12e8b47d05a6e21f9b3c7d84e50a1b6f2d9"
    );

    let r = tool(
        &app,
        "vercel-create-deployment",
        json!({"name": "storefront"}),
    )
    .await;
    assert_eq!(
        r,
        json!({"content": [{"type": "text", "text": "Failed to create deployment", "isError": true}]})
    );
    let r = tool(&app, "vercel-create-deployment", json!({"project": "storefront", "gitSource": {"type": "github", "ref": "fix/cart-badge"}, "target": "preview"})).await;
    let d: Value = serde_json::from_str(&texts(&r)[1]).unwrap();
    assert_eq!(d["target"], Value::Null);
    assert_eq!(
        d["meta"]["githubCommitSha"],
        "91b7e4c2d8a05f3e6b1c9d7a2e4f8b0c6d3a5e71"
    );
}

#[tokio::test]
async fn tool_project_team_and_custom_env_creation() {
    let w = world("storefront-production-status");
    let app = app(&w);
    let r = tool(
        &app,
        "vercel-create-project",
        json!({"name": "status-page"}),
    )
    .await;
    assert_eq!(r["content"][0]["isError"], true);
    assert!(texts(&r)[0].contains("\"teamId\""));
    let r = tool(
        &app,
        "vercel-create-project",
        json!({"name": "status-page", "teamId": ACME, "framework": "astro"}),
    )
    .await;
    assert!(texts(&r)[0].starts_with("Project status-page (prj_"));
    let r = tool(
        &app,
        "vercel-create-project",
        json!({"name": "status-page", "teamId": ACME}),
    )
    .await;
    assert_eq!(
        texts(&r),
        ["Project undefined (undefined) created successfully", "null"]
    );
    let r = tool(
        &app,
        "vercel-delete-project",
        json!({"idOrName": "status-page"}),
    )
    .await;
    assert_eq!(texts(&r), ["Project status-page deleted successfully"]);

    let r = tool(&app, "vercel-list-all-teams", json!({})).await;
    assert_eq!(texts(&r)[0], "Found 2 teams");
    let r = tool(&app, "vercel-create-team", json!({"slug": "acme-data"})).await;
    assert!(texts(&r)[0].starts_with("Team created successfully: acme-data (team_"));
    let r = tool(&app, "vercel-create-team", json!({"slug": "acme-data"})).await;
    assert_eq!(texts(&r), ["Failed to create team"]);

    let r = tool(
        &app,
        "vercel-create-custom-environment",
        json!({"idOrName": "storefront", "name": "Preview"}),
    )
    .await;
    assert_eq!(
        texts(&r),
        ["Invalid request: Custom environment name cannot be 'Production' or 'Preview'"]
    );
    let r = tool(&app, "vercel-create-custom-environment", json!({"idOrName": "storefront", "name": "qa", "branchMatcher": {"type": "startsWith", "pattern": "qa/"}})).await;
    assert_eq!(
        texts(&r)[0],
        "Successfully created custom environment 'qa' for project: storefront"
    );
    let r = tool(&app, "vercel-nope", json!({})).await;
    assert_eq!(texts(&r), ["Error: Unknown tool: vercel-nope"]);
}
