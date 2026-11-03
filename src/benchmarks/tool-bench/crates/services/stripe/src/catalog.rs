//! The pinned operation catalogue (`assets/operations.json`): what `stripe_api_search` and
//! `stripe_api_details` serve, and which operation ids the MCP accepts. Built from Stripe's
//! OpenAPI spec at API version 2026-08-26.dahlia via Kiln-AI/stripe_world (MIT); see
//! `assets/build_assets.py`.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

use serde_json::{Value, json};

/// What the remote MCP's discovery tools report as the spec version.
pub const DISCOVERY_SPEC_VERSION: &str = "2026-08-26.preview";
/// The API version objects and events are rendered at.
pub const API_VERSION: &str = "2026-08-26.dahlia";

const OPERATIONS: &str = include_str!("../assets/operations.json");
const OBJECTS: &str = include_str!("../assets/objects.json");

pub struct Operation {
    pub id: String,
    pub method: String,
    pub path: String,
    pub summary: String,
    /// Exposed through the MCP tools (false: REST only, e.g. v1 events).
    pub mcp: bool,
    pub doc: Value,
    tags: BTreeSet<String>,
    keywords: BTreeSet<String>,
    path_tokens: BTreeSet<String>,
    summary_lower: String,
}

impl Operation {
    /// Every top-level parameter name the operation accepts.
    pub fn param_names(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for group in ["path", "query", "body"] {
            if let Some(map) = self.doc["parameters"][group].as_object() {
                out.extend(map.keys().cloned());
            }
        }
        out
    }

    pub fn required_permissions(&self) -> Vec<String> {
        self.doc["required_permissions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `stripe_api_details` document.
    pub fn details(&self) -> Value {
        let mut doc = self.doc.clone();
        if let Some(map) = doc.as_object_mut() {
            map.remove("mcp");
            map.insert("openapi_spec_version".into(), json!(DISCOVERY_SPEC_VERSION));
        }
        doc
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(stem)
        .collect()
}

fn stem(token: &str) -> String {
    if token.len() > 3 && token.ends_with('s') && !token.ends_with("ss") {
        token[..token.len() - 1].to_owned()
    } else {
        token.to_owned()
    }
}

pub fn operations() -> &'static [Operation] {
    static OPS: OnceLock<Vec<Operation>> = OnceLock::new();
    OPS.get_or_init(|| {
        let docs: Vec<Value> = serde_json::from_str(OPERATIONS).expect("operations.json parses");
        docs.into_iter()
            .map(|doc| {
                let s = |k: &str| doc[k].as_str().unwrap_or_default().to_owned();
                let list = |k: &str| -> BTreeSet<String> {
                    doc[k]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .flat_map(tokens)
                                .collect()
                        })
                        .unwrap_or_default()
                };
                let path = s("path");
                Operation {
                    id: s("id"),
                    method: s("method"),
                    summary: s("summary"),
                    mcp: doc["mcp"].as_bool().unwrap_or(true),
                    tags: list("tags"),
                    keywords: list("keywords"),
                    path_tokens: path
                        .split('/')
                        .filter(|seg| !seg.starts_with('{'))
                        .flat_map(tokens)
                        .collect(),
                    summary_lower: s("summary").to_ascii_lowercase(),
                    path,
                    doc,
                }
            })
            .collect()
    })
}

pub fn find(id: &str) -> Option<&'static Operation> {
    operations().iter().find(|op| op.id == id)
}

/// Per object type: `nullable` and `expandable` field names from the spec.
pub fn objects() -> &'static BTreeMap<String, Value> {
    static OBJ: OnceLock<BTreeMap<String, Value>> = OnceLock::new();
    OBJ.get_or_init(|| serde_json::from_str(OBJECTS).expect("objects.json parses"))
}

pub fn nullable_fields(object: &str) -> Vec<&'static str> {
    objects()
        .get(object)
        .and_then(|o| o["nullable"].as_array())
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

pub fn expandable_fields(object: &str) -> Vec<&'static str> {
    objects()
        .get(object)
        .and_then(|o| o["expandable"].as_array())
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn intent_methods(term: &str) -> &'static [&'static str] {
    match term {
        "create" | "add" | "new" | "make" => &["POST"],
        "list" | "retrieve" | "get" | "read" | "find" | "fetch" | "show" | "view" | "search"
        | "lookup" => &["GET"],
        "update" | "modify" | "edit" | "change" | "set" | "apply" | "void" | "refund"
        | "finalize" | "capture" | "confirm" | "mark" => &["POST"],
        "cancel" => &["POST", "DELETE"],
        "delete" | "remove" => &["DELETE"],
        _ => &[],
    }
}

/// Ranked operations for an `intent` + `resource` query, as `{openapi_spec_version, data}`.
pub fn search(intent: &str, resource: &str, limit: usize) -> Value {
    let intent_terms = tokens(intent);
    let resource_terms = tokens(resource);
    let mut scored: Vec<(i64, &Operation)> = Vec::new();
    for op in operations().iter().filter(|op| op.mcp) {
        let mut score = 0;
        for term in &resource_terms {
            if op.tags.contains(term) {
                score += 3;
            }
            if op.keywords.contains(term) {
                score += 3;
            }
            if op.path_tokens.contains(term) {
                score += 2;
            }
        }
        if score == 0 && !resource_terms.is_empty() {
            continue;
        }
        for term in &intent_terms {
            if intent_methods(term).contains(&op.method.as_str()) {
                score += 2;
            }
            if op.summary_lower.contains(term.as_str()) {
                score += 1;
            }
            if op.keywords.contains(term) {
                score += 1;
            }
        }
        // Collection reads beat item reads for "list", item reads beat them for "retrieve".
        let item = op.path.contains('{');
        if intent_terms.iter().any(|t| t == "list") && !item && op.method == "GET" {
            score += 1;
        }
        if intent_terms.iter().any(|t| t == "retrieve" || t == "get") && item && op.method == "GET"
        {
            score += 1;
        }
        if score > 0 {
            scored.push((score, op));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    let data: Vec<Value> = scored
        .into_iter()
        .take(limit)
        .map(|(_, op)| {
            json!({
                "id": op.id,
                "method": op.method,
                "path": op.path,
                "summary": op.summary,
            })
        })
        .collect();
    json!({ "openapi_spec_version": DISCOVERY_SPEC_VERSION, "data": data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_loads_and_hides_rest_only_ops_from_search() {
        assert!(operations().len() > 120);
        assert!(find("PostRefunds").unwrap().mcp);
        assert!(!find("GetEvents").unwrap().mcp);
        let hits = search("list", "events", 20);
        assert!(
            hits["data"]
                .as_array()
                .unwrap()
                .iter()
                .all(|h| h["id"] != "GetEvents")
        );
    }

    #[test]
    fn search_ranks_the_obvious_operation_first() {
        let first = |i: &str, r: &str| search(i, r, 5)["data"][0]["id"].clone();
        assert_eq!(first("create", "refund"), "PostRefunds");
        assert_eq!(first("list", "customers"), "GetCustomers");
        assert_eq!(
            first("cancel", "subscription"),
            "DeleteSubscriptionsSubscriptionExposedId"
        );
    }
}
