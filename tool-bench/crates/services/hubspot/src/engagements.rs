//! Engagements v1 (`/engagements/v1/engagements`). Like HubSpot, v1 engagements are a view
//! over the CRM activity objects: a NOTE is the `notes` record with the same id, so notes
//! created here and through `/crm/v3/objects/notes` are the same thing.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use mock_core::{http::json, store::Store, world::ServiceState, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};

use crate::{
    assoc,
    misc::{account_details, token_info},
    objects::next_id,
    types::{ObjectType, resolve},
    util::{bad_json, iso_ms, iso_secs, not_found, parse_ms, validation, with_service},
};

/// `(v1 type, collection, [(metadata field, CRM property)])`
type Kind = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

const KINDS: &[Kind] = &[
    ("NOTE", "notes", &[("body", "hs_note_body")]),
    (
        "TASK",
        "tasks",
        &[
            ("body", "hs_task_body"),
            ("subject", "hs_task_subject"),
            ("status", "hs_task_status"),
            ("forObjectType", "hs_task_for_object_type"),
            ("priority", "hs_task_priority"),
            ("taskType", "hs_task_type"),
        ],
    ),
    (
        "CALL",
        "calls",
        &[
            ("body", "hs_call_body"),
            ("title", "hs_call_title"),
            ("status", "hs_call_status"),
            ("toNumber", "hs_call_to_number"),
            ("fromNumber", "hs_call_from_number"),
            ("durationMilliseconds", "hs_call_duration"),
            ("disposition", "hs_call_disposition"),
        ],
    ),
    (
        "EMAIL",
        "emails",
        &[
            ("subject", "hs_email_subject"),
            ("text", "hs_email_text"),
            ("html", "hs_email_html"),
            ("status", "hs_email_status"),
        ],
    ),
    (
        "MEETING",
        "meetings",
        &[
            ("body", "hs_meeting_body"),
            ("title", "hs_meeting_title"),
            ("startTime", "hs_meeting_start_time"),
            ("endTime", "hs_meeting_end_time"),
            ("location", "hs_meeting_location"),
            ("meetingOutcome", "hs_meeting_outcome"),
        ],
    ),
];

/// v1 association list -> CRM object type.
const ASSOCIATION_LISTS: &[(&str, &str)] = &[
    ("contactIds", "contacts"),
    ("companyIds", "companies"),
    ("dealIds", "deals"),
    ("ticketIds", "tickets"),
];

const DATETIME_FIELDS: &[&str] = &["hs_meeting_start_time", "hs_meeting_end_time"];

fn kind_by_type(t: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.0.eq_ignore_ascii_case(t))
}

fn find_engagement(store: &Store, id: &str) -> Option<(&'static str, Value)> {
    KINDS.iter().find_map(|(_, coll, _)| {
        store
            .collection(coll)
            .and_then(|c| c.get(id))
            .filter(|d| !d["archived"].as_bool().unwrap_or(false))
            .map(|d| (*coll, d.clone()))
    })
}

fn ms_of(v: &Value) -> Value {
    v.as_str()
        .and_then(parse_ms)
        .map_or(Value::Null, Value::from)
}

fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_owned()
}

fn metadata_value(prop: &str, v: &Value) -> Value {
    match (prop, v) {
        ("hs_call_duration", Value::String(s)) => s.parse::<i64>().map_or(v.clone(), Value::from),
        (p, Value::String(_)) if DATETIME_FIELDS.contains(&p) => ms_of(v),
        _ => v.clone(),
    }
}

/// The v1 representation of an activity record.
fn render(store: &Store, coll: &str, doc: &Value) -> Value {
    let Some((v1_type, _, fields)) = KINDS.iter().find(|k| k.1 == coll) else {
        return Value::Null;
    };
    let ty = resolve(store, coll).expect("activity types are standard");
    let props = doc["properties"].as_object().cloned().unwrap_or_default();
    let id = doc["id"].as_str().unwrap_or_default();
    let token = token_info(store);
    let portal = account_details(store)["portalId"].clone();
    let mut metadata = Map::new();
    for (field, prop) in *fields {
        if let Some(v) = props.get(*prop)
            && !v.is_null()
        {
            metadata.insert((*field).to_owned(), metadata_value(prop, v));
        }
    }
    let body = fields
        .iter()
        .find(|(f, _)| *f == "body" || *f == "text")
        .and_then(|(_, p)| props.get(*p))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut associations = Map::new();
    for (list, other) in ASSOCIATION_LISTS {
        let other_ty = resolve(store, other).expect("standard type");
        let ids: Vec<Value> = assoc::links(store, &ty, id, &other_ty)
            .into_iter()
            .map(|l| {
                l.to_id
                    .parse::<u64>()
                    .map_or(Value::String(l.to_id.clone()), Value::from)
            })
            .collect();
        associations.insert((*list).to_owned(), Value::Array(ids));
    }
    for empty in [
        "ownerIds",
        "workflowIds",
        "contentIds",
        "quoteIds",
        "marketingEventIds",
    ] {
        associations.insert(empty.to_owned(), json_value!([]));
    }
    let owner = props
        .get("hubspot_owner_id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<u64>().ok());
    let created = ms_of(&doc["createdAt"]);
    let updated = ms_of(&doc["updatedAt"]);
    let timestamp = props.get("hs_timestamp").map_or(created.clone(), ms_of);
    let preview: String = strip_html(body).chars().take(200).collect();
    json_value!({
        "engagement": {
            "id": id.parse::<u64>().map_or(Value::String(id.to_owned()), Value::from),
            "portalId": portal,
            "active": true,
            "createdAt": created,
            "lastUpdated": updated,
            "createdBy": token["userId"],
            "modifiedBy": token["userId"],
            "ownerId": owner,
            "type": v1_type,
            "timestamp": timestamp,
            "bodyPreview": preview,
            "queueMembershipIds": [],
            "bodyPreviewIsTruncated": strip_html(body).chars().count() > 200,
            "bodyPreviewHtml": body,
        },
        "associations": associations,
        "attachments": [],
        "metadata": metadata,
    })
}

/// Applies v1 metadata to CRM properties.
fn apply_metadata(
    fields: &[(&str, &str)],
    metadata: &Map<String, Value>,
    props: &mut Map<String, Value>,
) {
    for (field, prop) in fields {
        let Some(v) = metadata.get(*field) else {
            continue;
        };
        let value = match v {
            Value::Number(n) if DATETIME_FIELDS.contains(prop) => n
                .as_i64()
                .map_or(Value::Null, |ms| Value::String(iso_ms(ms))),
            Value::Number(n) => Value::String(n.to_string()),
            Value::Bool(b) => Value::String(b.to_string()),
            other => other.clone(),
        };
        props.insert((*prop).to_owned(), value);
    }
}

/// Validated association targets: `(object type, id)` for every listed id.
fn association_targets(
    svc: &mut ServiceState,
    associations: &Value,
) -> Result<Vec<(ObjectType, String)>, String> {
    let mut out = Vec::new();
    for (list, other) in ASSOCIATION_LISTS {
        let other_ty = resolve(&svc.store, other).expect("standard type");
        for id in associations[*list].as_array().into_iter().flatten() {
            let id = match id {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => continue,
            };
            if !assoc::object_exists(&svc.store, &other_ty, &id) {
                let message = format!("{} {id} does not exist", other_ty.singular.to_uppercase());
                return Err(message);
            }
            out.push((other_ty.clone(), id));
        }
    }
    Ok(out)
}

fn link(store: &mut Store, ty: &ObjectType, id: &str, targets: &[(ObjectType, String)]) {
    for (other, other_id) in targets {
        let defs = crate::types::association_defs(store, ty, other);
        if let Some(def) = defs.first() {
            let types = [(def.category.clone(), def.type_id)];
            assoc::add(store, ty, id, other, other_id, &types);
        }
    }
}

/// `POST /engagements/v1/engagements`
pub async fn create(State(world): State<SharedWorld>, body: Bytes) -> Response {
    with_service(&world, |svc, now| {
        let Ok(req) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        let Some(v1_type) = req["engagement"]["type"].as_str() else {
            return validation(svc, "engagement.type is required");
        };
        let Some((_, coll, fields)) = kind_by_type(v1_type) else {
            return validation(
                svc,
                &format!(
                    "Unsupported engagement type {v1_type}. Valid types: NOTE, TASK, CALL, EMAIL, MEETING"
                ),
            );
        };
        let ty = resolve(&svc.store, coll).expect("activity types are standard");
        let metadata = req["metadata"].as_object().cloned().unwrap_or_default();
        let targets = match association_targets(svc, &req["associations"]) {
            Ok(t) => t,
            Err(message) => return validation(svc, &message),
        };
        let ts = iso_secs(now);
        let timestamp = req["engagement"]["timestamp"]
            .as_i64()
            .map_or_else(|| ts.clone(), iso_ms);
        let mut props = Map::new();
        apply_metadata(fields, &metadata, &mut props);
        if *coll == "tasks" {
            props
                .entry("hs_task_status")
                .or_insert(json_value!("NOT_STARTED"));
        }
        props.insert("hs_timestamp".into(), Value::String(timestamp));
        if let Some(owner) = req["engagement"]["ownerId"].as_u64() {
            props.insert("hubspot_owner_id".into(), Value::String(owner.to_string()));
        }
        props.insert("hs_createdate".into(), Value::String(ts.clone()));
        props.insert("hs_lastmodifieddate".into(), Value::String(ts.clone()));
        let id = next_id(&svc.store, &ty);
        let doc = json_value!({
            "id": id,
            "properties": props,
            "createdAt": ts,
            "updatedAt": ts,
            "archived": false,
        });
        svc.store
            .collection_mut(coll)
            .insert(id.clone(), doc.clone());
        link(&mut svc.store, &ty, &id, &targets);
        json(StatusCode::OK, render(&svc.store, coll, &doc))
    })
}

/// `GET /engagements/v1/engagements/{engagementId}`
pub async fn get_one(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    with_service(&world, |svc, _| match find_engagement(&svc.store, &id) {
        Some((coll, doc)) => json(StatusCode::OK, render(&svc.store, coll, &doc)),
        None => not_found(svc, &format!("Engagement {id} does not exist")),
    })
}

/// `PATCH /engagements/v1/engagements/{engagementId}`: updates the given fields and adds
/// any listed associations.
pub async fn update(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    with_service(&world, |svc, now| {
        let Ok(req) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        let Some((coll, _)) = find_engagement(&svc.store, &id) else {
            return not_found(svc, &format!("Engagement {id} does not exist"));
        };
        let (_, _, fields) = KINDS.iter().find(|k| k.1 == coll).expect("known kind");
        let ty = resolve(&svc.store, coll).expect("activity types are standard");
        let targets = match association_targets(svc, &req["associations"]) {
            Ok(t) => t,
            Err(message) => return validation(svc, &message),
        };
        let ts = iso_secs(now);
        {
            let doc = svc
                .store
                .collection_mut(coll)
                .get_mut(&id)
                .expect("found above");
            if !doc["properties"].is_object() {
                doc["properties"] = json_value!({});
            }
            let props = doc["properties"].as_object_mut().expect("object");
            if let Some(metadata) = req["metadata"].as_object() {
                apply_metadata(fields, metadata, props);
            }
            if let Some(owner) = req["engagement"]["ownerId"].as_u64() {
                props.insert("hubspot_owner_id".into(), Value::String(owner.to_string()));
            }
            if let Some(t) = req["engagement"]["timestamp"].as_i64() {
                props.insert("hs_timestamp".into(), Value::String(iso_ms(t)));
            }
            props.insert("hs_lastmodifieddate".into(), Value::String(ts.clone()));
            doc["updatedAt"] = Value::String(ts);
        }
        link(&mut svc.store, &ty, &id, &targets);
        let doc = svc.store.collection(coll).and_then(|c| c.get(&id)).cloned();
        json(
            StatusCode::OK,
            render(&svc.store, coll, &doc.unwrap_or(Value::Null)),
        )
    })
}
