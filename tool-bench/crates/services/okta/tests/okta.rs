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
use mock_okta::Okta;
use serde_json::{Value, json};
use tower::ServiceExt;

const MARIA: &str = "00u1mrgnzlz6Wx3Yz4x7";
const ALEX_C: &str = "00u1alxkmct4St1Uv2x7";
const TAYLOR: &str = "00u1tylrbrk0Ef7Gh8x7";
const CHRIS: &str = "00u1chrsdz4Mn1Op2x7";
const PLATFORM: &str = "00g2pltfrm7Gh8Ij9x7";
const SUPPORT: &str = "00g2spprt12Ac5Ef6x7";
const SF_IMPORT: &str = "00g2sfimpt3Ab9Cd0x7";
const SALESFORCE: &str = "0oa3slsfrc5Lm8Rt1x7";
const PW_CONTRACT: &str = "00p4pwcntr2Uv9Wx0x7";
const PW_DEFAULT: &str = "00p4pwdflt0Pq7Rs8tx7";

fn world_for(scenario: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/cloud/okta")
        .join(scenario);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn setup() -> (SharedWorld, Router) {
    let w = world_for("offboard-contractor");
    let app = service_app(w.clone(), &Okta);
    (w, app)
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "octo-corp.okta.com")
        .header("authorization", "Bearer toolbench")
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(app: &Router, uri: &str) -> (StatusCode, HeaderMap, Value) {
    call(app, "GET", uri, None).await
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap().to_owned())
        .collect()
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["okta"].clone()
}

async fn form(app: &Router, uri: &str, body: &str) -> (StatusCode, Value) {
    let req = Request::post(uri)
        .header("host", "octo-corp.okta.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (
        status,
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn device_flow_completes_on_first_poll_with_a_long_lived_jwt() {
    let (_, app) = setup();
    let (status, device) = form(
        &app,
        "/oauth2/v1/device/authorize",
        "client_id=0oatoolbench&scope=openid",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        device["verification_uri"],
        "https://octo-corp.okta.com/activate"
    );
    let (status, token) = form(
        &app,
        "/oauth2/v1/token",
        &format!("client_id=0oatoolbench&device_code={}&grant_type=urn:ietf:params:oauth:grant-type:device_code", device["device_code"].as_str().unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let claims = mock_okta::decode_claims(token["access_token"].as_str().unwrap());
    assert_eq!(claims["exp"], 4_102_444_800_u64);
    assert_eq!(claims["iss"], "https://octo-corp.okta.com");
}

#[tokio::test]
async fn api_requires_a_token_and_unknown_endpoints_are_okta_404s() {
    let (_, app) = setup();
    let req = Request::get("/api/v1/users").body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["errorCode"], "E0000011");

    let (status, _, body) = get(&app, "/api/v1/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["errorCode"], "E0000007");
    let (status, _, body) = get(&app, "/api/v1/users/00unope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body["errorSummary"],
        "Not found: Resource not found: 00unope (User)"
    );
}

#[tokio::test]
async fn lists_users_with_okta_search_semantics() {
    let (_, app) = setup();
    let (status, headers, all) = get(&app, "/api/v1/users").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !ids(&all).contains(&TAYLOR.to_owned()),
        "deprovisioned users are hidden by default"
    );
    assert_eq!(all.as_array().unwrap().len(), 12);
    assert!(headers["link"].to_str().unwrap().contains("rel=\"self\""));
    assert_eq!(headers["x-rate-limit-limit"], "600");

    let (_, _, dep) = get(
        &app,
        "/api/v1/users?search=status%20eq%20%22DEPROVISIONED%22",
    )
    .await;
    assert_eq!(ids(&dep), [TAYLOR]);
    let (_, _, kim) = get(&app, "/api/v1/users?search=profile.lastName%20eq%20%22Kim%22%20and%20profile.userType%20eq%20%22Contractor%22").await;
    assert_eq!(ids(&kim), [ALEX_C]);
    let (_, _, q) = get(&app, "/api/v1/users?q=mar").await;
    assert_eq!(ids(&q), ["00u1mrcschn0Kl7Mn8x7", MARIA]);
    let (status, _, bad) = get(&app, "/api/v1/users?search=status%20zz%20%22x%22").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(bad["errorCode"], "E0000031");

    let (_, _, by_login) = get(&app, "/api/v1/users/maria.gonzalez@octo-corp.example").await;
    assert_eq!(by_login["id"], MARIA);
    assert_eq!(
        by_login["_links"]["self"]["href"],
        format!("https://octo-corp.okta.com/api/v1/users/{MARIA}")
    );
}

#[tokio::test]
async fn paginates_with_after_cursors_in_link_headers() {
    let (_, app) = setup();
    let (_, headers, page1) = get(&app, "/api/v1/users?limit=5").await;
    assert_eq!(page1.as_array().unwrap().len(), 5);
    let link = headers["link"].to_str().unwrap().to_owned();
    let last = ids(&page1).pop().unwrap();
    let next =
        format!("<https://octo-corp.okta.com/api/v1/users?after={last}&limit=5>; rel=\"next\"");
    assert!(link.contains(&next), "{link}");
    let (_, _, page2) = get(&app, &format!("/api/v1/users?after={last}&limit=5")).await;
    let (_, headers3, page3) = get(
        &app,
        &format!("/api/v1/users?after={}&limit=5", ids(&page2).pop().unwrap()),
    )
    .await;
    assert_eq!(page3.as_array().unwrap().len(), 2);
    assert!(!headers3["link"].to_str().unwrap().contains("rel=\"next\""));
}

#[tokio::test]
async fn creates_users_and_validates_like_okta() {
    let (w, app) = setup();
    let (status, _, u) = call(
        &app,
        "POST",
        "/api/v1/users?activate=false",
        Some(json!({ "profile": { "firstName": "Rae", "lastName": "Ng", "email": "rae.ng@octo-corp.example", "login": "rae.ng@octo-corp.example" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(u["status"], "STAGED");
    assert_eq!(u["created"], "2026-03-14T09:00:07.000Z");
    let id = u["id"].as_str().unwrap();
    assert!(id.starts_with("00u") && id.len() == 20);
    let s = state(&w);
    assert!(
        s["group_members"]
            .get(format!("00g1evrybdy0000000x1/{id}"))
            .is_some(),
        "joins Everyone"
    );
    assert_eq!(
        s["logs"].as_object().unwrap().values().next_back().unwrap()["eventType"],
        "user.lifecycle.create"
    );

    let (status, _, err) = call(
        &app,
        "POST",
        "/api/v1/users",
        Some(json!({ "profile": { "firstName": "X", "lastName": "Y", "email": "maria.gonzalez@octo-corp.example", "login": "maria.gonzalez@octo-corp.example" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorCode"], "E0000001");
    assert_eq!(err["errorSummary"], "Api validation failed: login");
    assert_eq!(
        err["errorCauses"][0]["errorSummary"],
        "login: An object with this field already exists in the current organization"
    );

    let (status, _, provisioned) = call(
        &app,
        "POST",
        "/api/v1/users",
        Some(json!({ "profile": { "firstName": "Bo", "lastName": "Lin", "email": "bo.lin@octo-corp.example", "login": "bo.lin@octo-corp.example" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        provisioned["status"], "PROVISIONED",
        "no password: activation email flow"
    );
}

#[tokio::test]
async fn partial_update_merges_the_profile() {
    let (w, app) = setup();
    let (status, _, u) = call(
        &app,
        "POST",
        &format!("/api/v1/users/{MARIA}"),
        Some(json!({ "profile": { "title": "Site Reliability Engineer" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(u["profile"]["title"], "Site Reliability Engineer");
    assert_eq!(u["profile"]["employeeNumber"], "1120");
    assert_eq!(state(&w)["users"][MARIA]["profile"]["lastName"], "Gonzalez");

    let (status, _, err) = call(
        &app,
        "PUT",
        &format!("/api/v1/users/{MARIA}"),
        Some(json!({ "profile": { "title": "x" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorCode"], "E0000001");
}

#[tokio::test]
async fn user_lifecycle_transitions_and_errors() {
    let (w, app) = setup();
    let (status, _, body) = call(
        &app,
        "POST",
        &format!("/api/v1/users/{ALEX_C}/lifecycle/deactivate"),
        None,
    )
    .await;
    assert_eq!((status, body), (StatusCode::OK, json!({})));
    let s = state(&w);
    assert_eq!(s["users"][ALEX_C]["status"], "DEPROVISIONED");
    let last = s["logs"]
        .as_object()
        .unwrap()
        .values()
        .next_back()
        .unwrap()
        .clone();
    assert_eq!(last["eventType"], "user.lifecycle.deactivate");
    assert_eq!(
        last["target"][0]["alternateId"],
        "alex.kim@northwind-consulting.example"
    );

    let (status, _, err) = call(
        &app,
        "POST",
        &format!("/api/v1/users/{ALEX_C}/lifecycle/suspend"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorCode"], "E0000001");
    let (status, _, err) = call(
        &app,
        "POST",
        &format!("/api/v1/users/{MARIA}/lifecycle/unlock"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(err["errorCode"], "E0000032");
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/users/{CHRIS}/lifecycle/unlock"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state(&w)["users"][CHRIS]["status"], "ACTIVE");
    let (status, _, err) = call(
        &app,
        "POST",
        &format!("/api/v1/users/{MARIA}/lifecycle/activate"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(err["errorCode"], "E0000016");

    // DELETE on a live user deactivates it; a second DELETE removes it.
    let (status, _, _) = call(&app, "DELETE", &format!("/api/v1/users/{MARIA}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(state(&w)["users"][MARIA]["status"], "DEPROVISIONED");
    let (status, _, _) = call(&app, "DELETE", &format!("/api/v1/users/{MARIA}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let s = state(&w);
    assert!(s["users"].get(MARIA).is_none());
    assert!(
        s["group_members"]
            .get(format!("{SUPPORT}/{MARIA}"))
            .is_none()
    );
}

#[tokio::test]
async fn group_membership_and_validation() {
    let (w, app) = setup();
    let (status, _, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/groups/{PLATFORM}/users/{MARIA}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/groups/{SUPPORT}/users/{MARIA}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let s = state(&w);
    assert!(
        s["group_members"]
            .get(format!("{PLATFORM}/{MARIA}"))
            .is_some()
    );
    assert!(
        s["group_members"]
            .get(format!("{SUPPORT}/{MARIA}"))
            .is_none()
    );
    let (_, _, groups) = get(&app, &format!("/api/v1/users/{MARIA}/groups")).await;
    assert_eq!(ids(&groups), ["00g1evrybdy0000000x1", PLATFORM]);
    let (_, _, members) = get(&app, &format!("/api/v1/groups/{SUPPORT}/users")).await;
    assert_eq!(ids(&members), [TAYLOR]);

    let (status, _, err) = call(
        &app,
        "PUT",
        &format!("/api/v1/groups/{SF_IMPORT}/users/{MARIA}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorCode"], "E0000060");
    let (status, _, err) = call(
        &app,
        "POST",
        "/api/v1/groups",
        Some(json!({ "profile": { "name": "Support Agents" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        err["errorCauses"][0]["errorSummary"],
        "name: An object with this field already exists in the current organization"
    );
    let (_, _, q) = get(&app, "/api/v1/groups?q=contr").await;
    assert_eq!(q.as_array().unwrap().len(), 2);
    let (_, _, apps) = get(&app, &format!("/api/v1/groups/{SUPPORT}/apps")).await;
    assert_eq!(apps[0]["label"], "Zendesk");
}

#[tokio::test]
async fn apps_filters_lifecycle_and_delete_rules() {
    let (w, app) = setup();
    let (_, _, for_group) = get(
        &app,
        "/api/v1/apps?filter=group.id%20eq%20%2200g2cntrsf6Op3Qr4x7%22",
    )
    .await;
    assert_eq!(ids(&for_group), [SALESFORCE]);
    let (_, _, for_user) = get(
        &app,
        &format!("/api/v1/apps?filter=user.id%20eq%20%22{ALEX_C}%22"),
    )
    .await;
    assert_eq!(
        ids(&for_user).len(),
        3,
        "Salesforce via group, plus Slack and the wiki via Everyone"
    );
    let (status, _, err) = call(&app, "DELETE", &format!("/api/v1/apps/{SALESFORCE}"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(err["errorCode"], "E0000056");
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/apps/{SALESFORCE}/lifecycle/deactivate"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(&app, "DELETE", &format!("/api/v1/apps/{SALESFORCE}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(state(&w)["apps"].get(SALESFORCE).is_none());

    let (status, _, created) = call(&app, "POST", "/api/v1/apps?activate=false", Some(json!({ "label": "Status Page", "signOnMode": "BOOKMARK", "settings": { "app": { "url": "https://status.octo-corp.example" } } }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["name"], "bookmark");
    assert_eq!(created["status"], "INACTIVE");
    let (status, _, err) = call(
        &app,
        "POST",
        "/api/v1/apps",
        Some(json!({ "signOnMode": "SAML_2_0" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorCode"], "E0000001");
    let (_, _, catalog) = get(&app, "/api/v1/catalog/apps?q=scim").await;
    assert_eq!(catalog[0]["name"], "scim2testapp");
}

#[tokio::test]
async fn policies_and_rules() {
    let (w, app) = setup();
    let (status, _, err) = get(&app, "/api/v1/policies").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorCode"], "E0000001");
    let (_, _, pw) = get(&app, "/api/v1/policies?type=PASSWORD").await;
    assert_eq!(ids(&pw), [PW_CONTRACT, "00p4pwempl4Yz1Ab2x7", PW_DEFAULT]);

    // A replace without settings/conditions keeps them (the SDK's base model drops them).
    let (status, _, p) = call(
        &app,
        "PUT",
        &format!("/api/v1/policies/{PW_CONTRACT}"),
        Some(json!({ "name": "Contractor passwords", "type": "PASSWORD" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["name"], "Contractor passwords");
    assert_eq!(
        state(&w)["policies"][PW_CONTRACT]["settings"]["password"]["complexity"]["minLength"],
        10
    );

    let rule_uri = format!("/api/v1/policies/{PW_CONTRACT}/rules/0pr5rule0tr2Uv9Wx0x7");
    let (status, _, r) = call(&app, "PUT", &rule_uri, Some(json!({ "type": "PASSWORD", "name": "Contractors rule", "actions": { "passwordChange": { "access": "ALLOW" }, "selfServicePasswordReset": { "access": "DENY" }, "selfServiceUnlock": { "access": "DENY" } } }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(r["actions"]["selfServicePasswordReset"]["access"], "DENY");
    assert_eq!(r["status"], "ACTIVE");
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("{rule_uri}/lifecycle/deactivate"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, r) = get(&app, &rule_uri).await;
    assert_eq!(r["status"], "INACTIVE");

    let (status, _, err) = call(
        &app,
        "DELETE",
        &format!("/api/v1/policies/{PW_DEFAULT}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorCode"], "E0000001");
    let (status, _, created) = call(
        &app,
        "POST",
        "/api/v1/policies",
        Some(json!({ "type": "PASSWORD", "name": "Interns" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["priority"], 3);
    assert_eq!(
        state(&w)["policies"][PW_DEFAULT]["priority"],
        4,
        "the default stays last"
    );
}

#[tokio::test]
async fn system_log_window_filter_order_and_cursor() {
    let w = world_for("failed-signins-report");
    let app = service_app(w, &Okta);
    let base = "/api/v1/logs?since=2026-03-13T00:00:00Z&until=2026-03-14T00:00:00.000Z";
    let (_, _, day) = get(&app, base).await;
    assert_eq!(day.as_array().unwrap().len(), 20);
    let (_, _, failures) = get(&app, &format!("{base}&filter=outcome.result%20eq%20%22FAILURE%22%20and%20actor.alternateId%20eq%20%22jordyn.patel%40octo-corp.example%22")).await;
    assert_eq!(failures.as_array().unwrap().len(), 6);
    assert!(
        failures
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["client"]["ipAddress"] == "203.0.113.77")
    );
    let (_, _, desc) = get(&app, &format!("{base}&sortOrder=DESCENDING&limit=1")).await;
    assert_eq!(desc[0]["published"], "2026-03-13T23:02:14.000Z");
    let (_, headers, first) = get(&app, &format!("{base}&limit=15")).await;
    assert_eq!(first.as_array().unwrap().len(), 15);
    assert!(headers["link"].to_str().unwrap().contains("rel=\"next\""));
    let (_, _, q) = get(&app, &format!("{base}&q=DENY")).await;
    assert_eq!(q.as_array().unwrap().len(), 4);
    // Default window: the last 7 days of scenario time.
    let (_, _, recent) = get(&app, "/api/v1/logs").await;
    assert_eq!(recent.as_array().unwrap().len(), 38);
    let (status, _, err) = get(&app, "/api/v1/logs?since=yesterday").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorCode"], "E0000001");
}

#[tokio::test]
async fn customizations_have_org_defaults_and_validate() {
    let (w, app) = setup();
    let (_, _, brands) = get(&app, "/api/v1/brands").await;
    let brand = brands[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(brands[0]["isDefault"], true);
    let (_, _, themes) = get(&app, &format!("/api/v1/brands/{brand}/themes")).await;
    let theme = themes[0]["id"].as_str().unwrap().to_owned();
    let (status, _, err) = call(
        &app,
        "PUT",
        &format!("/api/v1/brands/{brand}/themes/{theme}"),
        Some(json!({ "primaryColorHex": "blue" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorCode"], "E0000001");
    let (_, _, domains) = get(&app, "/api/v1/domains").await;
    assert_eq!(domains, json!({ "domains": [] }));
    let (_, _, brand_domains) = get(&app, &format!("/api/v1/brands/{brand}/domains")).await;
    assert_eq!(brand_domains, json!({ "domains": [] }));
    let (status, _, custom) = get(
        &app,
        &format!("/api/v1/brands/{brand}/pages/sign-in/customized"),
    )
    .await;
    assert_eq!(
        (status, custom["errorCode"].clone()),
        (StatusCode::NOT_FOUND, json!("E0000007"))
    );
    let (_, _, templates) = get(
        &app,
        &format!("/api/v1/brands/{brand}/templates/email?limit=100"),
    )
    .await;
    assert!(
        templates
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "UserActivation")
    );
    let (status, _, c) = call(
        &app,
        "POST",
        &format!("/api/v1/brands/{brand}/templates/email/UserActivation/customizations"),
        Some(json!({ "language": "en", "subject": "Welcome", "body": "<p>${activationLink}</p>" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(c["isDefault"], true);
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/brands/{brand}/templates/email/UserActivation/customizations"),
        Some(json!({ "language": "en", "subject": "Again", "body": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (_, _, ed) = call(&app, "POST", "/api/v1/email-domains", Some(json!({ "brandId": brand, "domain": "mail.octo-corp.example", "displayName": "IT", "userName": "it" }))).await;
    assert_eq!(ed["validationStatus"], "NOT_STARTED");
    let id = ed["id"].as_str().unwrap().to_owned();
    let (_, _, verified) = call(
        &app,
        "POST",
        &format!("/api/v1/email-domains/{id}/verify"),
        None,
    )
    .await;
    assert_eq!(verified["validationStatus"], "VERIFIED");
    assert_eq!(state(&w)["brands"][&brand]["emailDomainId"], id);

    let (status, _, da) = call(&app, "POST", "/api/v1/device-assurances", Some(json!({ "name": "macOS baseline", "platform": "MACOS", "osVersion": { "minimum": "14.0.0" } }))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, list) = get(&app, "/api/v1/device-assurances").await;
    assert_eq!(list[0]["id"], da["id"]);
    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/device-assurances/{}", da["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
