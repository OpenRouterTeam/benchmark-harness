//! CRM v3 properties: list, get, create and update property definitions.
//!
//! Built-in definitions come from code; created or dataset definitions live in the
//! `properties` collection under `<objectType>/<name>`.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};
use mock_core::{http::json, world::SharedWorld};
use serde_json::{Value, json as json_value};

use crate::{
    objects::unknown_type,
    types::{complete_property, property_definition, property_definitions, resolve},
    util::{bad_json, error, iso_secs, not_found, query_bool, validation, with_service},
};

const TYPES: &[&str] = &[
    "string",
    "number",
    "date",
    "datetime",
    "enumeration",
    "bool",
];
const FIELD_TYPES: &[&str] = &[
    "text",
    "textarea",
    "date",
    "file",
    "number",
    "select",
    "radio",
    "checkbox",
    "booleancheckbox",
    "calculation",
    "html",
    "phonenumber",
];
const WRITABLE: &[&str] = &[
    "label",
    "description",
    "groupName",
    "type",
    "fieldType",
    "options",
    "displayOrder",
    "hidden",
    "formField",
    "calculationFormula",
    "hasUniqueValue",
];

/// `GET /crm/v3/properties/{objectType}`
pub async fn list(
    State(world): State<SharedWorld>,
    Path(raw): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    with_service(&world, |svc, _| {
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        let archived = query_bool(&query, "archived");
        let include_hidden = query_bool(&query, "includeHidden");
        let results: Vec<Value> = property_definitions(&svc.store, &ty)
            .into_iter()
            .filter(|p| p["archived"].as_bool().unwrap_or(false) == archived)
            .filter(|p| include_hidden || !p["hidden"].as_bool().unwrap_or(false))
            .collect();
        json(StatusCode::OK, json_value!({ "results": results }))
    })
}

/// `GET /crm/v3/properties/{objectType}/{propertyName}`
pub async fn get_one(
    State(world): State<SharedWorld>,
    Path((raw, name)): Path<(String, String)>,
) -> Response {
    with_service(&world, |svc, _| {
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        match property_definition(&svc.store, &ty, &name) {
            Some(p) => json(StatusCode::OK, p),
            None => not_found(
                svc,
                &format!(
                    "Unable to find property {name} for object type {}",
                    ty.type_id
                ),
            ),
        }
    })
}

fn check_kinds(input: &Value) -> Result<(), String> {
    if let Some(t) = input.get("type").and_then(Value::as_str)
        && !TYPES.contains(&t)
    {
        return Err(format!(
            "Invalid property type {t}. Valid types: {}",
            TYPES.join(", ")
        ));
    }
    if let Some(f) = input.get("fieldType").and_then(Value::as_str)
        && !FIELD_TYPES.contains(&f)
    {
        return Err(format!(
            "Invalid field type {f}. Valid field types: {}",
            FIELD_TYPES.join(", ")
        ));
    }
    if let Some(options) = input.get("options") {
        let ok = options.as_array().is_some_and(|o| {
            o.iter()
                .all(|x| x["label"].is_string() && x["value"].is_string())
        });
        if !ok {
            return Err("Each option needs a string label and value".into());
        }
    }
    Ok(())
}

fn normalize_options(p: &mut Value) {
    if let Some(options) = p.get_mut("options").and_then(Value::as_array_mut) {
        for (i, o) in options.iter_mut().enumerate() {
            if let Some(obj) = o.as_object_mut() {
                obj.entry("description").or_insert(json_value!(""));
                obj.entry("displayOrder").or_insert(json_value!(i));
                obj.entry("hidden").or_insert(json_value!(false));
            }
        }
    }
}

/// `POST /crm/v3/properties/{objectType}`
pub async fn create(
    State(world): State<SharedWorld>,
    Path(raw): Path<String>,
    body: Bytes,
) -> Response {
    with_service(&world, |svc, now| {
        let Ok(input) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        for field in ["name", "label", "groupName", "type", "fieldType"] {
            if !input[field].is_string() {
                return validation(svc, &format!("Property {field} is required"));
            }
        }
        if let Err(message) = check_kinds(&input) {
            return validation(svc, &message);
        }
        let name = input["name"].as_str().unwrap_or_default().to_owned();
        let valid_name = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid_name {
            return validation(
                svc,
                &format!(
                    "Property name {name} is invalid: names must start with a lowercase letter and contain only lowercase letters, digits and underscores"
                ),
            );
        }
        if property_definition(&svc.store, &ty, &name).is_some() {
            return error(
                svc,
                StatusCode::CONFLICT,
                "OBJECT_ALREADY_EXISTS",
                &format!("A property named '{name}' already exists."),
            );
        }
        if input["type"] == "enumeration"
            && input["options"].as_array().is_none_or(Vec::is_empty)
            && input["fieldType"] != "booleancheckbox"
        {
            return validation(svc, "Enumeration properties require at least one option");
        }
        let ts = iso_secs(now);
        let mut p = json_value!({ "updatedAt": ts, "createdAt": ts, "name": name });
        for field in WRITABLE {
            if let Some(v) = input.get(*field) {
                p[*field] = v.clone();
            }
        }
        p["archived"] = json_value!(false);
        normalize_options(&mut p);
        let p = complete_property(p);
        svc.store
            .collection_mut("properties")
            .insert(format!("{}/{name}", ty.name), p.clone());
        json(StatusCode::CREATED, p)
    })
}

/// `PATCH /crm/v3/properties/{objectType}/{propertyName}`
pub async fn update(
    State(world): State<SharedWorld>,
    Path((raw, name)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    with_service(&world, |svc, now| {
        let Ok(input) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        let Some(mut p) = property_definition(&svc.store, &ty, &name) else {
            return not_found(
                svc,
                &format!(
                    "Unable to find property {name} for object type {}",
                    ty.type_id
                ),
            );
        };
        if p["modificationMetadata"]["readOnlyDefinition"] == true {
            return validation(
                svc,
                &format!(
                    "{name} is a HubSpot-defined property and its definition cannot be modified"
                ),
            );
        }
        if let Err(message) = check_kinds(&input) {
            return validation(svc, &message);
        }
        for field in WRITABLE {
            if let Some(v) = input.get(*field) {
                p[*field] = v.clone();
            }
        }
        p["updatedAt"] = Value::String(iso_secs(now));
        normalize_options(&mut p);
        svc.store
            .collection_mut("properties")
            .insert(format!("{}/{name}", ty.name), p.clone());
        json(StatusCode::OK, p)
    })
}
