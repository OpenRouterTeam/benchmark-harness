//! A deterministic in-memory SQL database for the Management API's `database/query`
//! endpoint and the analytics logs endpoint.

pub mod exec;
pub mod parse;
pub mod value;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use exec::{Db, Table, execute};

/// A Postgres error: SQLSTATE code plus message.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlError {
    pub code: &'static str,
    pub message: String,
}

impl SqlError {
    pub fn new(code: &'static str, message: String) -> Self {
        Self { code, message }
    }

    pub fn syntax_end() -> Self {
        Self::new("42601", "syntax error at end of input".to_owned())
    }

    /// The text the Management API puts in `message` for a failed query.
    pub fn api_message(&self) -> String {
        format!(
            "Failed to run sql query: ERROR:  {}: {}\n",
            self.code, self.message
        )
    }
}

fn default_true() -> bool {
    true
}

/// One column as stored in the `tables` collection of the dataset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    /// pg format name: int8, int4, text, varchar, bool, numeric, timestamptz, date, uuid,
    /// jsonb, or a user-defined enum type name (with `enums`).
    #[serde(rename = "type")]
    pub format: String,
    #[serde(default = "default_true")]
    pub nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unique: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enums: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    /// `schema.table`
    pub target: String,
    pub target_columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    #[serde(default)]
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyDef {
    pub name: String,
    pub command: String,
    pub roles: Vec<String>,
    #[serde(default)]
    pub using: Option<String>,
    #[serde(default)]
    pub check: Option<String>,
}

/// A table definition (`dataset/supabase/tables.json`, keyed `<ref>.<schema>.<name>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableDef {
    pub id: i64,
    pub schema: String,
    pub name: String,
    #[serde(default)]
    pub rls_enabled: bool,
    #[serde(default)]
    pub rls_forced: bool,
    #[serde(default)]
    pub comment: Option<String>,
    pub columns: Vec<ColumnInfo>,
    #[serde(default)]
    pub primary_keys: Vec<String>,
    #[serde(default)]
    pub foreign_keys: Vec<ForeignKey>,
    #[serde(default)]
    pub indexes: Vec<IndexDef>,
    #[serde(default)]
    pub policies: Vec<PolicyDef>,
}

impl TableDef {
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }
}

/// Rows of a result set as JSON objects, in column order.
pub type Rows = Vec<Value>;
