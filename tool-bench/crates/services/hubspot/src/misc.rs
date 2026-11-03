//! Account-level endpoints: private-app token info, account details, owners, custom
//! object schemas and automation (workflow) flows.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use mock_core::{http::json, store::Store, world::SharedWorld};
use serde_json::{Value, json as json_value};

use crate::{
    objects::unknown_type,
    types::resolve,
    util::{
        bad_json, base_url, id_order, not_found, page_window, paging, query_bool, query_one,
        validation, with_service,
    },
};

const DEFAULT_PORTAL: u64 = 24_681_357;
const DEFAULT_USER: u64 = 75_000_001;

fn account_doc<'a>(store: &'a Store, key: &str) -> Option<&'a Value> {
    store.collection("account").and_then(|c| c.get(key))
}

/// `account/details` from the dataset, falling back to a standard US portal.
pub fn account_details(store: &Store) -> Value {
    account_doc(store, "details").cloned().unwrap_or_else(|| {
        json_value!({
            "portalId": DEFAULT_PORTAL,
            "accountType": "STANDARD",
            "timeZone": "US/Eastern",
            "companyCurrency": "USD",
            "additionalCurrencies": [],
            "utcOffset": "-05:00",
            "utcOffsetMilliseconds": -18_000_000,
            "uiDomain": "app.hubspot.com",
            "dataHostingLocation": "na1"
        })
    })
}

/// `account/token` from the dataset; otherwise the first owner's user with full CRM scopes.
pub fn token_info(store: &Store) -> Value {
    if let Some(t) = account_doc(store, "token") {
        return t.clone();
    }
    let user = store
        .collection("owners")
        .and_then(|c| c.values().next())
        .and_then(|o| o["userId"].as_u64())
        .unwrap_or(DEFAULT_USER);
    json_value!({
        "userId": user,
        "hubId": account_details(store)["portalId"],
        "appId": 3_141_592,
        "scopes": [
            "automation",
            "crm.objects.companies.read",
            "crm.objects.companies.write",
            "crm.objects.contacts.read",
            "crm.objects.contacts.write",
            "crm.objects.custom.read",
            "crm.objects.custom.write",
            "crm.objects.deals.read",
            "crm.objects.deals.write",
            "crm.objects.owners.read",
            "crm.schemas.companies.read",
            "crm.schemas.companies.write",
            "crm.schemas.contacts.read",
            "crm.schemas.contacts.write",
            "crm.schemas.custom.read",
            "crm.schemas.deals.read",
            "crm.schemas.deals.write",
            "oauth",
            "tickets"
        ],
        "tokenType": "PRIVATE_APP"
    })
}

/// `POST /oauth/v2/private-apps/get/access-token-info`
pub async fn access_token_info(State(world): State<SharedWorld>, body: Bytes) -> Response {
    with_service(&world, |svc, _| {
        let Ok(req) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        if !req["tokenKey"].as_str().is_some_and(|t| !t.is_empty()) {
            return validation(svc, "tokenKey is required");
        }
        json(StatusCode::OK, token_info(&svc.store))
    })
}

/// `GET /account-info/v3/details`
pub async fn details(State(world): State<SharedWorld>) -> Response {
    with_service(&world, |svc, _| {
        json(StatusCode::OK, account_details(&svc.store))
    })
}

fn owners(store: &Store, archived: bool) -> Vec<Value> {
    let mut list: Vec<Value> = store
        .collection("owners")
        .map(|c| {
            c.values()
                .filter(|o| o["archived"].as_bool().unwrap_or(false) == archived)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    list.sort_by(|a, b| {
        id_order(
            a["id"].as_str().unwrap_or_default(),
            b["id"].as_str().unwrap_or_default(),
        )
    });
    list
}

/// `GET /crm/v3/owners`
pub async fn list_owners(
    State(world): State<SharedWorld>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    with_service(&world, |svc, _| {
        let (limit, after) = page_window(&query, 100, 500);
        let mut list = owners(&svc.store, query_bool(&query, "archived"));
        if let Some(email) = query_one(&query, "email") {
            list.retain(|o| {
                o["email"]
                    .as_str()
                    .is_some_and(|e| e.eq_ignore_ascii_case(email))
            });
        }
        let results: Vec<Value> = list.iter().skip(after).take(limit).cloned().collect();
        let mut body = json_value!({ "results": results });
        let link = format!("{}/crm/v3/owners/?limit={limit}", base_url(&headers));
        if let Some(p) = paging(list.len(), after, limit, Some(&link)) {
            body["paging"] = p;
        }
        json(StatusCode::OK, body)
    })
}

/// `GET /crm/v3/owners/{ownerId}?idProperty=id|userId`
pub async fn get_owner(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    with_service(&world, |svc, _| {
        let by_user = query_one(&query, "idProperty") == Some("userId");
        let found = owners(&svc.store, query_bool(&query, "archived"))
            .into_iter()
            .find(|o| {
                let field = if by_user { &o["userId"] } else { &o["id"] };
                match field {
                    Value::String(s) => *s == id,
                    Value::Number(n) => n.to_string() == id,
                    _ => false,
                }
            });
        match found {
            Some(o) => json(StatusCode::OK, o),
            None => not_found(svc, &format!("Owner {id} not found")),
        }
    })
}

/// `GET /crm-object-schemas/v3/schemas`
pub async fn list_schemas(State(world): State<SharedWorld>) -> Response {
    with_service(&world, |svc, _| {
        let results: Vec<Value> = svc
            .store
            .collection("schemas")
            .map(|c| {
                c.values()
                    .filter(|s| !s["archived"].as_bool().unwrap_or(false))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        json(StatusCode::OK, json_value!({ "results": results }))
    })
}

/// `GET /crm-object-schemas/v3/schemas/{objectType}`
pub async fn get_schema(State(world): State<SharedWorld>, Path(raw): Path<String>) -> Response {
    with_service(&world, |svc, _| {
        let Some(ty) = resolve(&svc.store, &raw).filter(|t| t.custom) else {
            return unknown_type(svc, &raw);
        };
        match crate::types::schema(&svc.store, &ty) {
            Some(s) => json(StatusCode::OK, s.clone()),
            None => unknown_type(svc, &raw),
        }
    })
}

const FLOW_SUMMARY: &[&str] = &[
    "id",
    "isEnabled",
    "flowType",
    "revisionId",
    "name",
    "createdAt",
    "updatedAt",
    "objectTypeId",
];

fn flows(store: &Store) -> Vec<Value> {
    let mut list: Vec<Value> = store
        .collection("flows")
        .map(|c| c.values().cloned().collect())
        .unwrap_or_default();
    list.sort_by(|a, b| {
        id_order(
            a["id"].as_str().unwrap_or_default(),
            b["id"].as_str().unwrap_or_default(),
        )
    });
    list
}

/// `GET /automation/v4/flows`
pub async fn list_flows(
    State(world): State<SharedWorld>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    with_service(&world, |svc, _| {
        let (limit, after) = page_window(&query, 100, 100);
        let all = flows(&svc.store);
        let results: Vec<Value> = all
            .iter()
            .skip(after)
            .take(limit)
            .map(|f| {
                let mut summary = serde_json::Map::new();
                for key in FLOW_SUMMARY {
                    if let Some(v) = f.get(*key) {
                        summary.insert((*key).to_owned(), v.clone());
                    }
                }
                Value::Object(summary)
            })
            .collect();
        let mut body = json_value!({ "results": results });
        let link = format!("{}/automation/v4/flows?limit={limit}", base_url(&headers));
        if let Some(p) = paging(all.len(), after, limit, Some(&link)) {
            body["paging"] = p;
        }
        json(StatusCode::OK, body)
    })
}

/// `GET /automation/v4/flows/{flowId}`
pub async fn get_flow(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    with_service(&world, |svc, _| {
        match svc
            .store
            .collection("flows")
            .and_then(|c| c.get(&id))
            .cloned()
        {
            Some(f) => json(StatusCode::OK, f),
            None => not_found(svc, &format!("Flow {id} not found")),
        }
    })
}
