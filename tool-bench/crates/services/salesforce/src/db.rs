//! A read view over the service store with the schema, plus Salesforce error values.

use std::sync::OnceLock;

use axum::http::StatusCode;
use mock_core::store::Store;
use serde_json::{Value, json};

use crate::{
    schema::{Object, Schema},
    util::{format_datetime, minted_id, normalize_id, parse_datetime},
};

/// One Salesforce API error (`[{"message", "errorCode", "fields"}]` on the wire).
#[derive(Debug, Clone, PartialEq)]
pub struct SfError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub fields: Vec<String>,
}

impl SfError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.to_owned(),
            message: message.into(),
            fields: Vec::new(),
        }
    }

    pub fn bad(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn with_fields(mut self, fields: &[&str]) -> Self {
        self.fields = fields.iter().map(|f| (*f).to_owned()).collect();
        self
    }

    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "The requested resource does not exist",
        )
    }

    /// REST body: `{"message", "errorCode", "fields"}`.
    pub fn rest(&self) -> Value {
        json!({ "message": self.message, "errorCode": self.code, "fields": self.fields })
    }

    /// sObject Collections body: `{"statusCode", "message", "fields"}`.
    pub fn collection(&self) -> Value {
        json!({ "statusCode": self.code, "message": self.message, "fields": self.fields })
    }
}

/// Read-only access to the org: schema, records, the logical clock and the API flavour.
pub struct Db<'a> {
    pub schema: &'a Schema,
    pub store: &'a Store,
    pub now_ms: i64,
    pub version: &'a str,
    pub tooling: bool,
}

impl<'a> Db<'a> {
    /// The object's records in dataset order (built-in defaults for orgs whose dataset has
    /// no profiles or permission sets).
    pub fn rows(&self, object: &str) -> Vec<&'a Value> {
        match self.store.collection(object) {
            Some(c) => c.values().collect(),
            None => builtin(object).iter().collect(),
        }
    }

    pub fn get(&self, object: &str, id: &str) -> Option<&'a Value> {
        let id = normalize_id(id)?;
        match self.store.collection(object) {
            Some(c) => c.get(&id),
            None => builtin(object).iter().find(|r| r["Id"] == id.as_str()),
        }
    }

    /// The record an Id points at, found through its key prefix.
    pub fn find(&self, id: &str) -> Option<(&'a Object, &'a Value)> {
        let object = self.schema.by_prefix(id)?;
        if let Some(rec) = self.get(&object.name, id) {
            return Some((object, rec));
        }
        // Several objects can share a prefix (the History objects); try them all.
        self.schema
            .objects
            .iter()
            .filter(|o| Some(o.prefix.as_str()) == id.get(..3))
            .find_map(|o| self.get(&o.name, id).map(|r| (o, r)))
    }

    pub fn sobject_url(&self, object: &str, id: &str) -> String {
        if self.tooling {
            format!(
                "/services/data/v{}/tooling/sobjects/{object}/{id}",
                self.version
            )
        } else {
            format!("/services/data/v{}/sobjects/{object}/{id}", self.version)
        }
    }

    pub fn attributes(&self, object: &str, rec: &Value) -> Value {
        match rec.get("Id").and_then(Value::as_str) {
            Some(id) => json!({ "type": object, "url": self.sobject_url(object, id) }),
            None => json!({ "type": object }),
        }
    }
}

/// The API user: the first active user in the dataset.
pub fn current_user(store: &Store) -> Option<&Value> {
    store
        .collection("User")?
        .values()
        .find(|u| u["IsActive"] != false)
}

pub fn current_user_id(store: &Store) -> Value {
    current_user(store)
        .map(|u| u["Id"].clone())
        .unwrap_or(Value::Null)
}

const PROFILES: &[&str] = &[
    "System Administrator",
    "Standard User",
    "Read Only",
    "Marketing User",
    "Contract Manager",
    "Solution Manager",
    "Standard Platform User",
];

fn builtin(object: &str) -> &'static [Value] {
    static PROFILE_ROWS: OnceLock<Vec<Value>> = OnceLock::new();
    static PERMSET_ROWS: OnceLock<Vec<Value>> = OnceLock::new();
    let created = || format_datetime(parse_datetime("2024-01-15T18:22:41Z").expect("valid"));
    match object {
        "Profile" => PROFILE_ROWS.get_or_init(|| {
            PROFILES
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    json!({
                        "Id": minted_id("00e", 1000 + i as u64),
                        "Name": name,
                        "UserType": "Standard",
                        "Description": null,
                        "CreatedDate": created(),
                        "CreatedById": null,
                        "LastModifiedDate": created(),
                        "LastModifiedById": null,
                        "SystemModstamp": created(),
                    })
                })
                .collect()
        }),
        "PermissionSet" => PERMSET_ROWS.get_or_init(|| {
            PROFILES
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let profile = minted_id("00e", 1000 + i as u64);
                    json!({
                        "Id": minted_id("0PS", 1000 + i as u64),
                        "Name": format!("X{}", &profile[..15]),
                        "Label": profile[..15].to_owned(),
                        "IsOwnedByProfile": true,
                        "ProfileId": profile,
                        "Description": format!("Permission set owned by the {name} profile"),
                        "CreatedDate": created(),
                        "CreatedById": null,
                        "LastModifiedDate": created(),
                        "LastModifiedById": null,
                        "SystemModstamp": created(),
                    })
                })
                .collect()
        }),
        _ => &[],
    }
}

/// Built-in rows for collections the dataset doesn't define, copied into the store before
/// the first write to them.
pub fn builtin_rows(object: &str) -> &'static [Value] {
    builtin(object)
}
