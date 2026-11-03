//! Metadata API (`POST /services/Soap/m/{version}`): `createMetadata`, `readMetadata`,
//! `updateMetadata`, `upsertMetadata` and `deleteMetadata` for `CustomObject` and
//! `CustomField`. Definitions are stored in the `CustomObject` / `CustomField` collections
//! (keyed by full name) and overlaid onto describe and SOQL, so a created field is
//! immediately queryable. Other operations return the SOAP fault Salesforce sends for an
//! unknown operation.

use axum::{
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use mock_core::world::ServiceState;
use serde_json::{Map, Value, json};

use crate::{
    schema::{CUSTOM_FIELDS, CUSTOM_OBJECTS, Schema},
    util::{XmlNode, escape_xml, parse_xml},
};

const NS: &str = "http://soap.sforce.com/2006/04/metadata";

fn envelope(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\" xmlns=\"{NS}\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><soapenv:Body>{body}</soapenv:Body></soapenv:Envelope>"
    )
}

fn xml_response(status: StatusCode, body: String) -> Response {
    (status, [(CONTENT_TYPE, "text/xml;charset=UTF-8")], body).into_response()
}

pub fn fault(code: &str, message: &str) -> Response {
    let body = format!(
        "<soapenv:Fault><faultcode>{}</faultcode><faultstring>{}</faultstring></soapenv:Fault>",
        escape_xml(code),
        escape_xml(message)
    );
    xml_response(StatusCode::INTERNAL_SERVER_ERROR, envelope(&body))
}

/// Element tree to JSON: leaves become strings, repeated children arrays.
fn to_json(n: &XmlNode) -> Value {
    if n.children.is_empty() {
        return if n.is_nil() {
            Value::Null
        } else {
            Value::String(n.text.trim().to_owned())
        };
    }
    let mut m = Map::new();
    for c in &n.children {
        let v = to_json(c);
        match m.get_mut(&c.name) {
            Some(Value::Array(a)) => a.push(v),
            Some(existing) => {
                let prev = existing.take();
                *existing = Value::Array(vec![prev, v]);
            }
            None => {
                m.insert(c.name.clone(), v);
            }
        }
    }
    Value::Object(m)
}

fn to_xml(name: &str, v: &Value, out: &mut String) {
    match v {
        Value::Array(items) => items.iter().for_each(|i| to_xml(name, i, out)),
        Value::Object(m) => {
            out.push_str(&format!("<{name}>"));
            for (k, v) in m {
                to_xml(k, v, out);
            }
            out.push_str(&format!("</{name}>"));
        }
        Value::Null => out.push_str(&format!("<{name} xsi:nil=\"true\"/>")),
        Value::String(s) => out.push_str(&format!("<{name}>{}</{name}>", escape_xml(s))),
        other => out.push_str(&format!("<{name}>{other}</{name}>")),
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|x| !x.is_empty())
}

struct SaveError {
    code: &'static str,
    message: String,
    field: Option<&'static str>,
}

fn err(code: &'static str, message: impl Into<String>) -> SaveError {
    SaveError {
        code,
        message: message.into(),
        field: None,
    }
}

fn valid_api_name(base: &str) -> bool {
    let mut chars = base.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !base.contains("__")
        && !base.ends_with('_')
        && base.len() <= 40
}

fn validate_object(md: &Value) -> Result<(), SaveError> {
    let full = s(md, "fullName").ok_or_else(|| {
        err(
            "REQUIRED_FIELD_MISSING",
            "Required field is missing: fullName",
        )
    })?;
    let Some(base) = full.strip_suffix("__c") else {
        return Err(err(
            "FIELD_INTEGRITY_EXCEPTION",
            format!("Custom object names must end with __c: {full}"),
        ));
    };
    if !valid_api_name(base) {
        return Err(err(
            "FIELD_INTEGRITY_EXCEPTION",
            format!(
                "The Object Name {base} can only contain underscores and alphanumeric characters. It must be unique, begin with a letter, not include spaces, not end with an underscore, and not contain two consecutive underscores."
            ),
        ));
    }
    for (k, label) in [
        ("label", "label"),
        ("pluralLabel", "pluralLabel"),
        ("nameField", "nameField"),
    ] {
        if md.get(k).is_none_or(|v| v.is_null() || v == "") {
            return Err(SaveError {
                field: Some(label),
                ..err(
                    "FIELD_INTEGRITY_EXCEPTION",
                    format!("Must specify '{label}' for a CustomObject"),
                )
            });
        }
    }
    Ok(())
}

fn validate_field(schema: &Schema, md: &Value) -> Result<(), SaveError> {
    let full = s(md, "fullName").ok_or_else(|| {
        err(
            "REQUIRED_FIELD_MISSING",
            "Required field is missing: fullName",
        )
    })?;
    let Some((object, field)) = full.split_once('.') else {
        return Err(err(
            "FIELD_INTEGRITY_EXCEPTION",
            format!("Invalid fullName, must be of the form Object.Field: {full}"),
        ));
    };
    if schema.get_in(object, false).is_none() {
        return Err(err(
            "INVALID_CROSS_REFERENCE_KEY",
            format!("In field: fullName - no CustomObject named {object} found"),
        ));
    }
    let Some(base) = field.strip_suffix("__c") else {
        return Err(err(
            "FIELD_INTEGRITY_EXCEPTION",
            format!("Custom field names must end with __c: {field}"),
        ));
    };
    if !valid_api_name(base) {
        return Err(err(
            "FIELD_INTEGRITY_EXCEPTION",
            format!(
                "The Field Name {base} can only contain underscores and alphanumeric characters. It must be unique, begin with a letter, not include spaces, not end with an underscore, and not contain two consecutive underscores."
            ),
        ));
    }
    let kind = s(md, "type").ok_or_else(|| {
        err(
            "FIELD_INTEGRITY_EXCEPTION",
            "Must specify 'type' for a CustomField",
        )
    })?;
    if s(md, "label").is_none() {
        return Err(err(
            "FIELD_INTEGRITY_EXCEPTION",
            "Must specify 'label' for a CustomField",
        ));
    }
    let need = |k: &'static str| -> Result<(), SaveError> {
        if md.get(k).is_none_or(Value::is_null) {
            Err(SaveError {
                field: Some(k),
                ..err(
                    "FIELD_INTEGRITY_EXCEPTION",
                    format!("Must specify '{k}' for a CustomField of type {kind}"),
                )
            })
        } else {
            Ok(())
        }
    };
    match kind.as_str() {
        "Text" | "LongTextArea" | "Html" | "EncryptedText" => need("length")?,
        "Number" | "Currency" | "Percent" => need("precision")?,
        "Checkbox" => need("defaultValue")?,
        "Picklist" | "MultiselectPicklist" => need("valueSet")?,
        "Lookup" | "MasterDetail" => {
            need("referenceTo")?;
            need("relationshipName")?;
            let to = s(md, "referenceTo").unwrap_or_default();
            if schema.get_in(&to, false).is_none() {
                return Err(err(
                    "FIELD_INTEGRITY_EXCEPTION",
                    format!("referenceTo value of '{to}' does not resolve to a valid sObject type"),
                ));
            }
        }
        "Text Area" => {
            return Err(err(
                "FIELD_INTEGRITY_EXCEPTION",
                format!("'{kind}' is not a valid value for the enum 'FieldType'"),
            ));
        }
        "TextArea" | "Date" | "DateTime" | "Email" | "Phone" | "Url" | "AutoNumber" => {}
        other => {
            return Err(err(
                "FIELD_INTEGRITY_EXCEPTION",
                format!("'{other}' is not a valid value for the enum 'FieldType'"),
            ));
        }
    }
    if kind == "Text"
        && s(md, "length")
            .and_then(|l| l.parse::<u32>().ok())
            .is_some_and(|l| l == 0 || l > 255)
    {
        return Err(err(
            "FIELD_INTEGRITY_EXCEPTION",
            "Length must be between 1 and 255",
        ));
    }
    Ok(())
}

fn collection_for(kind: &str) -> Option<&'static str> {
    match kind {
        "CustomObject" => Some(CUSTOM_OBJECTS),
        "CustomField" => Some(CUSTOM_FIELDS),
        _ => None,
    }
}

fn save_result(full: &str, outcome: Result<(), SaveError>, extra: &str) -> String {
    match outcome {
        Ok(()) => format!(
            "<result>{extra}<fullName>{}</fullName><success>true</success></result>",
            escape_xml(full)
        ),
        Err(e) => format!(
            "<result>{extra}<errors>{}<message>{}</message><statusCode>{}</statusCode></errors><fullName>{}</fullName><success>false</success></result>",
            e.field
                .map(|f| format!("<fields>{f}</fields>"))
                .unwrap_or_default(),
            escape_xml(&e.message),
            e.code,
            escape_xml(full)
        ),
    }
}

fn exists(state: &ServiceState, schema: &Schema, kind: &str, full: &str) -> bool {
    match kind {
        "CustomObject" => state
            .store
            .collection(CUSTOM_OBJECTS)
            .is_some_and(|c| c.iter().any(|(k, _)| k.eq_ignore_ascii_case(full))),
        _ => {
            state
                .store
                .collection(CUSTOM_FIELDS)
                .is_some_and(|c| c.iter().any(|(k, _)| k.eq_ignore_ascii_case(full)))
                || full
                    .split_once('.')
                    .is_some_and(|(o, f)| schema.get(o).is_some_and(|o| o.has_field(f)))
        }
    }
}

fn store_key(state: &ServiceState, collection: &str, full: &str) -> String {
    state
        .store
        .collection(collection)
        .and_then(|c| {
            c.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(full))
                .map(|(k, _)| k.to_owned())
        })
        .unwrap_or_else(|| full.to_owned())
}

fn save(
    state: &mut ServiceState,
    kind: &str,
    md: &Value,
    mode: &str,
) -> (String, Result<(), SaveError>, bool) {
    let full = s(md, "fullName").unwrap_or_default();
    let Some(collection) = collection_for(kind) else {
        return (
            full,
            Err(err(
                "INVALID_TYPE",
                format!("Cannot {mode} metadata of type {kind} in this organization"),
            )),
            false,
        );
    };
    let schema = Schema::load(&state.store);
    let present = exists(state, &schema, kind, &full);
    let create = match mode {
        "create" if present => {
            let msg = if kind == "CustomObject" {
                format!("There is already a Custom Object named {full}")
            } else {
                let (o, f) = full.split_once('.').unwrap_or(("", &full));
                format!(
                    "There is already a field named {} on {o}.",
                    f.trim_end_matches("__c")
                )
            };
            return (full, Err(err("DUPLICATE_DEVELOPER_NAME", msg)), false);
        }
        "update" if !present => {
            let what = if kind == "CustomObject" {
                "CustomObject"
            } else {
                "CustomField"
            };
            let msg = format!("In field: fullName - no {what} named {full} found");
            return (full, Err(err("INVALID_CROSS_REFERENCE_KEY", msg)), false);
        }
        "create" => true,
        "update" => false,
        _ => !present,
    };
    let check = if kind == "CustomObject" {
        validate_object(md)
    } else {
        validate_field(&schema, md)
    };
    if let Err(e) = check {
        return (full, Err(e), create);
    }
    if kind == "CustomField"
        && !create
        && !state
            .store
            .collection(CUSTOM_FIELDS)
            .is_some_and(|c| c.iter().any(|(k, _)| k.eq_ignore_ascii_case(&full)))
    {
        let msg = format!("Cannot update standard field {full}");
        return (full, Err(err("FIELD_INTEGRITY_EXCEPTION", msg)), false);
    }
    let key = store_key(state, collection, &full);
    let mut doc = md.clone();
    if let Value::Object(m) = &mut doc {
        m.insert("fullName".into(), json!(key));
    }
    state.store.collection_mut(collection).insert(key, doc);
    (full, Ok(()), create)
}

/// Handles one Metadata API envelope.
pub fn handle(state: &mut ServiceState, body: &str) -> Response {
    let Ok(root) = parse_xml(body) else {
        return fault(
            "soapenv:Client",
            "Element type \"soapenv:Envelope\" must be followed by either attribute specifications, \">\" or \"/>\".",
        );
    };
    let session = root
        .child("Header")
        .and_then(|h| h.child("SessionHeader"))
        .and_then(|s| s.child("sessionId"))
        .map(|s| s.text.trim().to_owned())
        .unwrap_or_default();
    if session.is_empty() {
        return fault(
            "sf:INVALID_SESSION_ID",
            "INVALID_SESSION_ID: Invalid Session ID found in SessionHeader: Illegal Session",
        );
    }
    let Some(op) = root.child("Body").and_then(|b| b.children.first()) else {
        return fault("soapenv:Client", "No operation found in SOAP Body");
    };
    let name = op.name.as_str();
    let mut results = String::new();
    match name {
        "createMetadata" | "updateMetadata" | "upsertMetadata" => {
            let mode = &name[..6];
            for md in op.children_named("metadata") {
                let kind = md.xsi_type().unwrap_or("").to_owned();
                let (full, outcome, created) = save(state, &kind, &to_json(md), mode);
                let extra = if mode == "upsert" {
                    format!("<created>{created}</created>")
                } else {
                    String::new()
                };
                results.push_str(&save_result(&full, outcome, &extra));
            }
        }
        "readMetadata" => {
            let kind = op
                .child("type")
                .map(|t| t.text.trim().to_owned())
                .unwrap_or_default();
            let mut records = String::new();
            if let Some(collection) = collection_for(&kind) {
                for full in op.children_named("fullNames") {
                    let full = full.text.trim();
                    if let Some(doc) = state.store.collection(collection).and_then(|c| {
                        c.iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case(full))
                            .map(|(_, v)| v.clone())
                    }) {
                        let mut inner = String::new();
                        if let Value::Object(m) = &doc {
                            for (k, v) in m {
                                to_xml(k, v, &mut inner);
                            }
                        }
                        records
                            .push_str(&format!("<records xsi:type=\"{kind}\">{inner}</records>"));
                    }
                }
            }
            results = format!("<result>{records}</result>");
        }
        "deleteMetadata" => {
            let kind = op
                .child("type")
                .map(|t| t.text.trim().to_owned())
                .unwrap_or_default();
            for full in op.children_named("fullNames") {
                let full = full.text.trim().to_owned();
                let outcome = match collection_for(&kind) {
                    Some(collection) => {
                        let key = store_key(state, collection, &full);
                        if state
                            .store
                            .collection_mut(collection)
                            .remove(&key)
                            .is_some()
                        {
                            Ok(())
                        } else {
                            Err(err(
                                "INVALID_CROSS_REFERENCE_KEY",
                                format!("In field: fullName - no {kind} named {full} found"),
                            ))
                        }
                    }
                    None => Err(err(
                        "INVALID_TYPE",
                        format!("Cannot delete metadata of type {kind} in this organization"),
                    )),
                };
                results.push_str(&save_result(&full, outcome, ""));
            }
        }
        other => {
            return fault(
                "soapenv:Client",
                &format!(
                    "No operation available for request {{{NS}}}{other}, please check the WSDL for the service."
                ),
            );
        }
    }
    xml_response(
        StatusCode::OK,
        envelope(&format!("<{name}Response>{results}</{name}Response>")),
    )
}
