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
use mock_hubspot::Hubspot;
use serde_json::{Value, json};
use tower::ServiceExt;

fn world_from(rel: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

/// The shared CRM world used by the HubSpot scenarios.
fn world() -> SharedWorld {
    world_from("../../../scenarios/business/hubspot/dana-joins-pied-piper")
}

fn custom_world() -> SharedWorld {
    world_from("tests/fixtures/custom-objects")
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Value,
    auth: bool,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8123")
        .header("content-type", "application/json");
    if auth {
        req = req.header("authorization", "Bearer pat-na1-toolbench");
    }
    let payload = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(payload)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    send(app, "GET", uri, Value::Null, true).await
}

async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    send(app, "POST", uri, body, true).await
}

fn ids(v: &Value) -> Vec<String> {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect()
}

fn assert_error(body: &Value, category: &str) {
    assert_eq!(body["status"], "error", "{body}");
    assert_eq!(body["category"], category, "{body}");
    assert!(body["message"].is_string());
    assert_eq!(body["correlationId"].as_str().unwrap().len(), 36);
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["hubspot"].clone()
}

// --- account, token, owners -------------------------------------------------------------

#[tokio::test]
async fn requires_a_bearer_token() {
    let app = service_app(world(), &Hubspot);
    let (status, body) = send(&app, "GET", "/crm/v3/objects/contacts", Value::Null, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["category"], "INVALID_AUTHENTICATION");
}

#[tokio::test]
async fn user_details_endpoints() {
    let app = service_app(world(), &Hubspot);
    let (status, token) = post(
        &app,
        "/oauth/v2/private-apps/get/access-token-info",
        json!({"tokenKey": "pat-na1-toolbench"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(token["userId"], 75000001);
    assert_eq!(token["hubId"], 24681357);
    assert!(token["scopes"].as_array().unwrap().len() > 5);

    let (status, bad) = post(
        &app,
        "/oauth/v2/private-apps/get/access-token-info",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error(&bad, "VALIDATION_ERROR");

    let (_, account) = get(&app, "/account-info/v3/details").await;
    assert_eq!(account["portalId"], 24681357);
    assert_eq!(account["uiDomain"], "app.hubspot.com");

    let (status, owner) = get(
        &app,
        "/crm/v3/owners/75000001?idProperty=userId&archived=false",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(owner["id"], "81234501");
    assert_eq!(owner["email"], "priya.raman@northbeam.io");

    let (status, missing) = get(&app, "/crm/v3/owners/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&missing, "OBJECT_NOT_FOUND");
}

#[tokio::test]
async fn lists_owners_with_paging_and_email_filter() {
    let app = service_app(world(), &Hubspot);
    let (_, page) = get(&app, "/crm/v3/owners?limit=3").await;
    assert_eq!(ids(&page), ["81234501", "81234502", "81234503"]);
    assert_eq!(page["paging"]["next"]["after"], "3");
    let (_, rest) = get(&app, "/crm/v3/owners?limit=3&after=3").await;
    assert_eq!(ids(&rest), ["81234504"]);
    assert!(rest.get("paging").is_none());
    let (_, by_email) = get(&app, "/crm/v3/owners?email=Elena.Sokolova@northbeam.io").await;
    assert_eq!(ids(&by_email), ["81234503"]);
}

// --- objects: list / get ----------------------------------------------------------------

#[tokio::test]
async fn lists_objects_with_default_properties_and_paging() {
    let app = service_app(world(), &Hubspot);
    let (status, page) = get(&app, "/crm/v3/objects/contacts?limit=4&archived=false").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&page), ["512001", "512002", "512003", "512004"]);
    let first = &page["results"][0];
    let keys: Vec<&str> = first["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "createdate",
            "email",
            "firstname",
            "hs_object_id",
            "lastmodifieddate",
            "lastname"
        ]
    );
    assert_eq!(first["properties"]["hs_object_id"], "512001");
    assert_eq!(first["archived"], false);
    assert_eq!(page["paging"]["next"]["after"], "4");
    assert_eq!(
        page["paging"]["next"]["link"],
        "http://world:8123/crm/v3/objects/contacts?limit=4&archived=false&after=4"
    );

    let (_, last) = get(&app, "/crm/v3/objects/contacts?limit=4&after=8").await;
    assert_eq!(ids(&last), ["512009", "512010"]);
    assert!(last.get("paging").is_none());
}

#[tokio::test]
async fn list_selects_properties_and_associations_and_hides_archived() {
    let app = service_app(world(), &Hubspot);
    let (_, deals) = get(
        &app,
        "/crm/v3/objects/0-3?limit=100&properties=dealname,hubspot_owner_id,hs_priority&associations=companies",
    )
    .await;
    assert_eq!(deals["results"].as_array().unwrap().len(), 13);
    let d = &deals["results"][0];
    assert_eq!(
        d["properties"]["dealname"],
        "Globex – Platform Expansion 2027"
    );
    assert_eq!(d["properties"]["hubspot_owner_id"], "81234502");
    assert_eq!(d["properties"]["hs_priority"], Value::Null);
    assert!(d["properties"].get("amount").is_none());
    assert_eq!(
        d["associations"]["companies"]["results"],
        json!([{"id": "18234001", "type": "deal_to_company_unlabeled"}, {"id": "18234001", "type": "deal_to_company"}])
    );

    let (_, archived) = get(&app, "/crm/v3/objects/deals?archived=true").await;
    assert_eq!(ids(&archived), ["9100114"]);
    assert!(archived["results"][0]["archivedAt"].is_string());
}

#[tokio::test]
async fn get_by_id_and_real_errors() {
    let app = service_app(world(), &Hubspot);
    let (status, c) = get(&app, "/crm/v3/objects/contacts/512005?properties=jobtitle").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(c["properties"]["jobtitle"], "VP Engineering");
    let (_, by_email) = get(
        &app,
        "/crm/v3/objects/contacts/richard@piedpiper.com?idProperty=email",
    )
    .await;
    assert_eq!(by_email["id"], "512004");

    let (status, missing) = get(&app, "/crm/v3/objects/contacts/1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&missing, "OBJECT_NOT_FOUND");

    let (status, bad) = get(&app, "/crm/v3/objects/dealz").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error(&bad, "VALIDATION_ERROR");
    assert_eq!(bad["message"], "Unable to infer object type from: dealz");

    let (status, route) = get(&app, "/crm/v3/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(route["category"], "OBJECT_NOT_FOUND");
}

// --- search -----------------------------------------------------------------------------

async fn search(app: &Router, ty: &str, body: Value) -> Value {
    let (status, v) = post(app, &format!("/crm/v3/objects/{ty}/search"), body).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    v
}

#[tokio::test]
async fn search_filters_sorts_and_pages() {
    let app = service_app(world(), &Hubspot);
    let open = search(
        &app,
        "deals",
        json!({"filterGroups": [{"filters": [{"propertyName": "dealstage", "operator": "NOT_IN", "values": ["closedwon", "closedlost"]}]}],
               "sorts": [{"propertyName": "amount", "direction": "DESCENDING"}], "properties": ["amount"], "limit": 2}),
    )
    .await;
    assert_eq!(open["total"], 8);
    assert_eq!(ids(&open), ["9100106", "9100108"]);
    assert_eq!(open["results"][0]["properties"]["amount"], "412000");
    assert_eq!(open["paging"]["next"]["after"], "2");

    let page2 = search(
        &app,
        "deals",
        json!({"filterGroups": [{"filters": [{"propertyName": "dealstage", "operator": "NOT_IN", "values": ["closedwon", "closedlost"]}]}],
               "sorts": [{"propertyName": "amount", "direction": "DESCENDING"}], "limit": 2, "after": "2"}),
    )
    .await;
    assert_eq!(ids(&page2), ["9100104", "9100110"]);

    let between = search(
        &app,
        "deals",
        json!({"filterGroups": [{"filters": [
            {"propertyName": "amount", "operator": "BETWEEN", "value": "100000", "highValue": "180000"},
            {"propertyName": "closedate", "operator": "GT", "value": "2026-12-01"}
        ]}]}),
    )
    .await;
    assert_eq!(ids(&between), ["9100110", "9100111", "9100112"]);

    let either = search(
        &app,
        "deals",
        json!({"filterGroups": [
            {"filters": [{"propertyName": "hubspot_owner_id", "operator": "EQ", "value": "81234504"}]},
            {"filters": [{"propertyName": "amount", "operator": "GTE", "value": 600000}]}
        ]}),
    )
    .await;
    assert_eq!(ids(&either), ["9100109", "9100112"]);

    let by_company = search(
        &app,
        "deals",
        json!({"filterGroups": [{"filters": [{"propertyName": "associations.company", "operator": "EQ", "value": "18234001"}]}]}),
    )
    .await;
    assert_eq!(ids(&by_company), ["9100101", "9100102", "9100103"]);
}

#[tokio::test]
async fn search_query_tokens_and_presence() {
    let app = service_app(world(), &Hubspot);
    let danas = search(&app, "contacts", json!({"query": "dana whit"})).await;
    assert_eq!(ids(&danas), ["512001", "512002", "512003"]);

    let domain = search(
        &app,
        "contacts",
        json!({"filterGroups": [{"filters": [{"propertyName": "email", "operator": "CONTAINS_TOKEN", "value": "*@piedpiper.com"}]}]}),
    )
    .await;
    assert_eq!(ids(&domain), ["512003", "512004"]);

    let companies = search(
        &app,
        "companies",
        json!({"filterGroups": [{"filters": [{"propertyName": "name", "operator": "CONTAINS_TOKEN", "value": "piper"}]}], "properties": ["name", "domain"]}),
    )
    .await;
    assert_eq!(ids(&companies), ["18234004", "18234005"]);
    assert_eq!(
        companies["results"][1]["properties"]["domain"],
        "piedpiperpizza.com"
    );

    let none = search(
        &app,
        "deals",
        json!({"filterGroups": [{"filters": [{"propertyName": "hs_priority", "operator": "HAS_PROPERTY"}]}]}),
    )
    .await;
    assert_eq!(none["total"], 0);
    assert!(none.get("paging").is_none());

    let (status, bad) = post(
        &app,
        "/crm/v3/objects/deals/search",
        json!({"filterGroups": [{"filters": [{"propertyName": "amount", "operator": "ABOUT", "value": "1"}]}]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error(&bad, "VALIDATION_ERROR");
}

// --- batch read / update / create -------------------------------------------------------

#[tokio::test]
async fn batch_read_reports_missing_ids() {
    let app = service_app(world(), &Hubspot);
    let (status, body) = post(
        &app,
        "/crm/v3/objects/deals/batch/read",
        json!({"inputs": [{"id": "9100104"}, {"id": "42"}], "properties": ["dealname"], "propertiesWithHistory": ["dealstage"]}),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(body["status"], "COMPLETE");
    assert_eq!(ids(&body), ["9100104"]);
    assert_eq!(
        body["results"][0]["propertiesWithHistory"]["dealstage"][0]["value"],
        "decisionmakerboughtin"
    );
    assert_eq!(body["numErrors"], 1);
    assert_eq!(body["errors"][0]["category"], "OBJECT_NOT_FOUND");
    assert_eq!(body["errors"][0]["context"]["ids"], json!(["42"]));

    let (status, by_email) = post(
        &app,
        "/crm/v3/objects/contacts/batch/read",
        json!({"inputs": [{"id": "hank@globex.com"}], "idProperty": "email"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&by_email), ["512006"]);
}

#[tokio::test]
async fn batch_update_validates_and_writes_state() {
    let w = world();
    let app = service_app(w.clone(), &Hubspot);
    let (status, body) = post(
        &app,
        "/crm/v3/objects/deals/batch/update",
        json!({"inputs": [{"id": "9100101", "properties": {"dealstage": "contractsent", "closedate": "2026-11-20"}}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "COMPLETE");
    let r = &body["results"][0];
    assert_eq!(r["properties"]["dealstage"], "contractsent");
    assert_eq!(r["properties"]["closedate"], "2026-11-20T00:00:00.000Z");
    assert_eq!(r["updatedAt"], "2026-10-29T14:00:07.000Z");

    let s = state(&w);
    assert_eq!(
        s["deals"]["9100101"]["properties"]["dealstage"],
        "contractsent"
    );
    assert_eq!(
        s["deals"]["9100101"]["properties"]["hs_lastmodifieddate"],
        "2026-10-29T14:00:07.000Z"
    );

    let (status, bad) = post(
        &app,
        "/crm/v3/objects/deals/batch/update",
        json!({"inputs": [{"id": "9100102", "properties": {"dealstage": "Contract Sent", "colour": "red", "amount": "lots"}}]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error(&bad, "VALIDATION_ERROR");
    let message = bad["message"].as_str().unwrap();
    assert!(
        message.starts_with("Property values were not valid: "),
        "{message}"
    );
    for code in ["INVALID_OPTION", "PROPERTY_DOESNT_EXIST", "INVALID_FLOAT"] {
        assert!(message.contains(code), "{message}");
    }
    assert_eq!(
        state(&w)["deals"]["9100102"]["properties"]["dealstage"],
        "closedwon"
    );

    let (status, partial) = post(
        &app,
        "/crm/v3/objects/contacts/batch/update",
        json!({"inputs": [{"id": "512001", "properties": {"email": "dana.whitfield@piedpiper.com"}}, {"id": "1", "properties": {"email": "x@y.z"}}]}),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(ids(&partial), ["512001"]);
    assert_eq!(partial["errors"][0]["category"], "OBJECT_NOT_FOUND");
    assert_eq!(
        state(&w)["contacts"]["512001"]["properties"]["email"],
        "dana.whitfield@piedpiper.com"
    );
}

#[tokio::test]
async fn batch_create_assigns_ids_and_associations() {
    let w = world();
    let app = service_app(w.clone(), &Hubspot);
    let (status, body) = post(
        &app,
        "/crm/v3/objects/contacts/batch/create",
        json!({"inputs": [{"properties": {"email": "jared@piedpiper.com", "firstname": "Jared", "lastname": "Dunn"},
                           "associations": [{"to": {"id": "18234004"}, "types": [{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 279}]}]}]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let created = &body["results"][0];
    assert_eq!(created["id"], "512011");
    assert_eq!(created["properties"]["firstname"], "Jared");
    assert_eq!(created["createdAt"], "2026-10-29T14:00:07.000Z");
    let s = state(&w);
    assert_eq!(
        s["contacts"]["512011"]["properties"]["email"],
        "jared@piedpiper.com"
    );
    assert_eq!(
        s["associations"]["companies/18234004/contacts/512011"]["types"],
        json!([{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 280}])
    );

    let (status, dup) = post(
        &app,
        "/crm/v3/objects/contacts/batch/create",
        json!({"inputs": [{"properties": {"email": "JARED@piedpiper.com"}}]}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_error(&dup, "CONFLICT");
    assert_eq!(
        dup["message"],
        "Contact already exists. Existing ID: 512011"
    );

    let (status, no_ts) = post(
        &app,
        "/crm/v3/objects/notes/batch/create",
        json!({"inputs": [{"properties": {"hs_note_body": "hi"}}]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        no_ts["message"]
            .as_str()
            .unwrap()
            .contains("REQUIRED_FIELD")
    );

    let (status, note) = post(
        &app,
        "/crm/v3/objects/notes/batch/create",
        json!({"inputs": [{"properties": {"hs_note_body": "Pricing call", "hs_timestamp": "1793282400000"},
                           "associations": [{"to": {"id": "9100104"}, "types": [{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 214}]}]}]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{note}");
    assert_eq!(
        note["results"][0]["id"], "61000005",
        "activities share one id space"
    );
    let s = state(&w);
    assert_eq!(
        s["notes"]["61000005"]["properties"]["hs_timestamp"],
        "2026-10-29T14:00:00.000Z"
    );
    assert!(s["associations"]["deals/9100104/notes/61000005"].is_object());
}

// --- properties -------------------------------------------------------------------------

#[tokio::test]
async fn property_definitions_crud() {
    let w = world();
    let app = service_app(w.clone(), &Hubspot);
    let (_, list) = get(
        &app,
        "/crm/v3/properties/deals?archived=false&includeHidden=false",
    )
    .await;
    let names: Vec<&str> = list["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"dealstage") && names.contains(&"deal_source"));

    let (_, stage) = get(&app, "/crm/v3/properties/deals/dealstage").await;
    assert_eq!(stage["type"], "enumeration");
    assert_eq!(stage["options"][4]["value"], "contractsent");
    assert_eq!(stage["hubspotDefined"], true);

    let (status, missing) = get(&app, "/crm/v3/properties/deals/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&missing, "OBJECT_NOT_FOUND");

    let def = json!({"name": "renewal_risk", "label": "Renewal risk", "groupName": "dealinformation", "type": "enumeration", "fieldType": "select",
                     "options": [{"label": "Low", "value": "low"}, {"label": "High", "value": "high"}]});
    let (status, created) = post(&app, "/crm/v3/properties/deals", def.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["createdAt"], "2026-10-29T14:00:07.000Z");
    assert_eq!(created["options"][1]["displayOrder"], 1);
    assert_eq!(
        state(&w)["properties"]["deals/renewal_risk"]["label"],
        "Renewal risk"
    );
    let (status, conflict) = post(&app, "/crm/v3/properties/deals", def).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_error(&conflict, "OBJECT_ALREADY_EXISTS");

    let (status, updated) = send(
        &app,
        "PATCH",
        "/crm/v3/properties/deals/renewal_risk",
        json!({"label": "Renewal risk (CSM)"}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["label"], "Renewal risk (CSM)");

    let (status, _) = send(
        &app,
        "PATCH",
        "/crm/v3/properties/deals/dealstage",
        json!({"label": "Stage"}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The new property is now writable on deals.
    let (status, _) = post(
        &app,
        "/crm/v3/objects/deals/batch/update",
        json!({"inputs": [{"id": "9100104", "properties": {"renewal_risk": "high"}}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// --- associations -----------------------------------------------------------------------

#[tokio::test]
async fn association_labels_and_listing() {
    let app = service_app(world(), &Hubspot);
    let (_, labels) = get(&app, "/crm/v4/associations/contacts/companies/labels").await;
    assert_eq!(
        labels["results"],
        json!([{"category": "HUBSPOT_DEFINED", "typeId": 279, "label": null}, {"category": "HUBSPOT_DEFINED", "typeId": 1, "label": "Primary"}])
    );
    let (_, list) = get(
        &app,
        "/crm/v4/objects/contacts/512001/associations/companies?limit=500",
    )
    .await;
    assert_eq!(
        list["results"],
        json!([{"toObjectId": 18234003, "associationTypes": [
            {"category": "HUBSPOT_DEFINED", "typeId": 279, "label": null},
            {"category": "HUBSPOT_DEFINED", "typeId": 1, "label": "Primary"}
        ]}])
    );
    let (_, deals) = get(
        &app,
        "/crm/v4/objects/companies/18234003/associations/deals?limit=2",
    )
    .await;
    assert_eq!(deals["results"].as_array().unwrap().len(), 2);
    assert_eq!(deals["paging"]["next"]["after"], "2");
    let (_, none) = get(&app, "/crm/v4/objects/contacts/1/associations/companies").await;
    assert_eq!(none["results"], json!([]));
}

#[tokio::test]
async fn batch_create_associations_moves_primary_company() {
    let w = world();
    let app = service_app(w.clone(), &Hubspot);
    let (status, body) = post(
        &app,
        "/crm/v4/associations/contacts/companies/batch/create",
        json!({"inputs": [{"from": {"id": "512001"}, "to": {"id": "18234004"}, "types": [{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 1}]}]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        body["results"],
        json!([{"fromObjectTypeId": "0-1", "fromObjectId": 512001, "toObjectTypeId": "0-2", "toObjectId": 18234004, "labels": ["Primary"]}])
    );
    let s = state(&w);
    assert_eq!(
        s["associations"]["companies/18234004/contacts/512001"]["types"],
        json!([
            {"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 2},
            {"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 280}
        ])
    );
    assert_eq!(
        s["associations"]["companies/18234003/contacts/512001"]["types"],
        json!([{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 280}]),
        "Hooli is no longer the primary company"
    );

    let (status, partial) = post(
        &app,
        "/crm/v4/associations/companies/contacts/batch/create",
        json!({"inputs": [{"from": {"id": "18234005"}, "to": {"id": "999"}, "types": [{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 280}]}]}),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(partial["numErrors"], 1);
    assert_eq!(partial["results"], json!([]));

    let (status, bad) = post(
        &app,
        "/crm/v4/associations/contacts/companies/batch/create",
        json!({"inputs": [{"from": {"id": "512002"}, "to": {"id": "18234004"}, "types": [{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 3}]}]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error(&bad, "VALIDATION_ERROR");
}

// --- engagements ------------------------------------------------------------------------

#[tokio::test]
async fn engagements_v1_create_get_update() {
    let w = world();
    let app = service_app(w.clone(), &Hubspot);
    let (status, created) = post(
        &app,
        "/engagements/v1/engagements",
        json!({"engagement": {"active": true, "ownerId": 81234501, "type": "NOTE", "timestamp": 1793282400000_i64},
               "associations": {"contactIds": [512005], "companyIds": [], "dealIds": [9100104], "ownerIds": [], "ticketIds": []},
               "metadata": {"body": "Security review passed."}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["engagement"]["id"], 61000005);
    assert_eq!(created["engagement"]["type"], "NOTE");
    assert_eq!(created["engagement"]["ownerId"], 81234501);
    assert_eq!(created["engagement"]["timestamp"], 1793282400000_i64);
    assert_eq!(created["engagement"]["createdAt"], 1793282407000_i64);
    assert_eq!(created["associations"]["dealIds"], json!([9100104]));
    assert_eq!(created["associations"]["contactIds"], json!([512005]));
    assert_eq!(
        created["metadata"],
        json!({"body": "Security review passed."})
    );

    let s = state(&w);
    assert_eq!(
        s["notes"]["61000005"]["properties"]["hs_note_body"],
        "Security review passed."
    );
    assert!(s["associations"]["deals/9100104/notes/61000005"].is_object());
    assert!(s["associations"]["contacts/512005/notes/61000005"].is_object());

    // The same note is visible as a CRM object.
    let (_, as_object) = get(
        &app,
        "/crm/v3/objects/notes/61000005?properties=hs_note_body",
    )
    .await;
    assert_eq!(
        as_object["properties"]["hs_note_body"],
        "Security review passed."
    );

    let (status, task) = get(&app, "/engagements/v1/engagements/61000004").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(task["engagement"]["type"], "TASK");
    assert_eq!(task["metadata"]["status"], "NOT_STARTED");
    assert_eq!(task["associations"]["dealIds"], json!([9100101]));

    let (status, updated) = send(
        &app,
        "PATCH",
        "/engagements/v1/engagements/61000004",
        json!({"metadata": {"status": "COMPLETED"}, "associations": {"contactIds": [512006], "companyIds": [], "dealIds": [], "ownerIds": [], "ticketIds": []}}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["metadata"]["status"], "COMPLETED");
    assert_eq!(updated["associations"]["dealIds"], json!([9100101]));
    assert_eq!(updated["associations"]["contactIds"], json!([512006]));

    let (status, missing) = get(&app, "/engagements/v1/engagements/77").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&missing, "OBJECT_NOT_FOUND");

    let (status, bad) = post(
        &app,
        "/engagements/v1/engagements",
        json!({"engagement": {"type": "NOTE", "ownerId": 1}, "associations": {"dealIds": [1]}, "metadata": {"body": "x"}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error(&bad, "VALIDATION_ERROR");
}

// --- workflows, schemas, custom objects -------------------------------------------------

#[tokio::test]
async fn workflows_list_and_get() {
    let app = service_app(world(), &Hubspot);
    let (_, list) = get(&app, "/automation/v4/flows?limit=1").await;
    assert_eq!(ids(&list), ["1590341"]);
    assert!(
        list["results"][0].get("actions").is_none(),
        "list is a summary"
    );
    assert_eq!(list["paging"]["next"]["after"], "1");
    let (_, flow) = get(&app, "/automation/v4/flows/1590388").await;
    assert_eq!(flow["name"], "New contact: assign owner round-robin");
    assert!(flow["actions"].is_array());
    let (status, missing) = get(&app, "/automation/v4/flows/1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&missing, "OBJECT_NOT_FOUND");
}

#[tokio::test]
async fn custom_object_schemas_and_records() {
    let w = custom_world();
    let app = service_app(w.clone(), &Hubspot);
    let (_, schemas) = get(&app, "/crm-object-schemas/v3/schemas").await;
    assert_eq!(
        schemas["results"][0]["fullyQualifiedName"],
        "p24681357_projects"
    );

    let (_, list) = get(&app, "/crm/v3/objects/projects").await;
    assert_eq!(ids(&list), ["30001", "30002"]);
    assert_eq!(
        list["results"][0]["properties"]["project_name"],
        "Warehouse cutover"
    );

    let found = search(&app, "2-8812345", json!({"query": "db-07"})).await;
    assert_eq!(ids(&found), ["30002"]);

    let (_, props) = get(&app, "/crm/v3/properties/p24681357_projects").await;
    assert!(
        props["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "budget")
    );

    let (status, created) = post(
        &app,
        "/crm/v3/objects/projects/batch/create",
        json!({"inputs": [{"properties": {"project_name": "Telemetry pilot", "budget": "30000"},
                           "associations": [{"to": {"id": "18234002"}, "types": [{"associationCategory": "USER_DEFINED", "associationTypeId": 512}]}]}]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["results"][0]["id"], "30003");
    let (_, linked) = get(
        &app,
        "/crm/v4/objects/companies/18234002/associations/projects",
    )
    .await;
    assert_eq!(
        linked["results"],
        json!([{"toObjectId": 30003, "associationTypes": [{"category": "USER_DEFINED", "typeId": 513, "label": null}]}])
    );
}
