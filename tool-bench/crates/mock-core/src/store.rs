use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde_json::{Map, Value};

/// Named JSON documents in insertion order.
#[derive(Debug, Clone, Default)]
pub struct Collection {
    docs: IndexMap<String, Value>,
}

impl Collection {
    pub fn insert(&mut self, key: impl Into<String>, doc: Value) -> Option<Value> {
        self.docs.insert(key.into(), doc)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.docs.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.docs.get_mut(key)
    }

    /// Removes a document, keeping the order of the rest.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.docs.shift_remove(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.docs.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.docs.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.docs.values()
    }
}

/// All collections of one service.
#[derive(Debug, Clone, Default)]
pub struct Store {
    collections: BTreeMap<String, Collection>,
}

impl Store {
    pub fn collection(&self, name: &str) -> Option<&Collection> {
        self.collections.get(name)
    }

    pub fn collection_mut(&mut self, name: &str) -> &mut Collection {
        self.collections.entry(name.to_owned()).or_default()
    }

    /// `{ "<collection>": { "<key>": doc, ... }, ... }`, collections sorted by name.
    pub fn snapshot(&self) -> Value {
        let mut out = Map::new();
        for (name, collection) in &self.collections {
            let docs = collection
                .iter()
                .map(|(k, v)| (k.to_owned(), v.clone()))
                .collect::<Map<_, _>>();
            out.insert(name.clone(), Value::Object(docs));
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keeps_insertion_order_and_removal_preserves_it() {
        let mut c = Collection::default();
        c.insert("b", json!({"n": 1}));
        c.insert("a", json!({"n": 2}));
        c.insert("c", json!({"n": 3}));
        assert_eq!(c.remove("a"), Some(json!({"n": 2})));
        let keys: Vec<&str> = c.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["b", "c"]);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn get_mut_changes_document() {
        let mut c = Collection::default();
        c.insert("x", json!({"state": "open"}));
        c.get_mut("x").unwrap()["state"] = json!("closed");
        assert_eq!(c.get("x"), Some(&json!({"state": "closed"})));
        assert!(c.contains("x"));
        assert!(!c.contains("y"));
    }

    #[test]
    fn collection_mut_creates_and_snapshot_is_sorted_by_collection() {
        let mut s = Store::default();
        assert!(s.collection("repos").is_none());
        s.collection_mut("repos").insert("o/r", json!({"id": 1}));
        s.collection_mut("issues")
            .insert("o/r#1", json!({"number": 1}));
        assert_eq!(
            serde_json::to_string(&s.snapshot()).unwrap(),
            r#"{"issues":{"o/r#1":{"number":1}},"repos":{"o/r":{"id":1}}}"#
        );
    }
}
