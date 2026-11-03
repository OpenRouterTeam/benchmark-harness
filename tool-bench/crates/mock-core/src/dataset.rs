use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    clock::LogicalClock,
    error::{Error, Result},
    store::{Collection, Store},
};

/// The `[scenario]` table of `scenario.toml`. Other tables are ignored here.
#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioMeta {
    pub id: String,
    pub now: String,
    pub seed: String,
    #[serde(default = "default_step")]
    pub clock_step_secs: i64,
}

fn default_step() -> i64 {
    LogicalClock::DEFAULT_STEP_SECS
}

#[derive(Deserialize)]
struct ScenarioFile {
    scenario: ScenarioMeta,
}

/// A scenario's starting world: metadata plus one store per service directory.
#[derive(Debug, Clone)]
pub struct Dataset {
    pub meta: ScenarioMeta,
    pub services: BTreeMap<String, Store>,
}

impl Dataset {
    /// Loads `<root>/scenario.toml` and every `<root>/dataset/<service>/<collection>.json`.
    pub fn load(root: &Path) -> Result<Self> {
        let meta_path = root.join("scenario.toml");
        let file: ScenarioFile = toml::from_str(&read(&meta_path)?).map_err(|e| Error::Parse {
            path: meta_path.clone(),
            message: e.to_string(),
        })?;

        let mut services = BTreeMap::new();
        let data_dir = root.join("dataset");
        if data_dir.is_dir() {
            for service_dir in sorted_entries(&data_dir)? {
                if !service_dir.is_dir() {
                    continue;
                }
                let mut store = Store::default();
                for file in sorted_entries(&service_dir)? {
                    if file.extension().is_some_and(|ext| ext == "json") {
                        let name = file_stem(&file);
                        load_collection(&file, store.collection_mut(&name))?;
                    }
                }
                services.insert(file_stem(&service_dir), store);
            }
        }

        Ok(Self {
            meta: file.scenario,
            services,
        })
    }
}

fn load_collection(path: &Path, collection: &mut Collection) -> Result<()> {
    let value: Value = serde_json::from_str(&read(path)?).map_err(|e| Error::Parse {
        path: path.to_owned(),
        message: e.to_string(),
    })?;
    let Value::Array(docs) = value else {
        return Err(dataset_error(
            path,
            "expected a JSON array of documents".to_owned(),
        ));
    };
    for (index, mut doc) in docs.into_iter().enumerate() {
        let key = document_key(&mut doc).ok_or_else(|| {
            dataset_error(path, format!("document {index} has no \"_key\" or \"id\""))
        })?;
        if collection.insert(key.clone(), doc).is_some() {
            return Err(dataset_error(path, format!("duplicate key {key:?}")));
        }
    }
    Ok(())
}

/// `_key` (removed from the document) wins; otherwise the `id` field as a string.
fn document_key(doc: &mut Value) -> Option<String> {
    let obj = doc.as_object_mut()?;
    if let Some(key) = obj.shift_remove("_key") {
        return Some(match key {
            Value::String(s) => s,
            other => other.to_string(),
        });
    }
    match obj.get("id")? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}

fn sorted_entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let io = |source| Error::Io {
        path: dir.to_owned(),
        source,
    };
    let mut paths = fs::read_dir(dir)
        .map_err(io)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(io)?;
    paths.sort();
    Ok(paths)
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn dataset_error(path: &Path, message: String) -> Error {
    Error::Dataset {
        path: path.to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    fn example_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/example/github/basic")
    }

    #[test]
    fn loads_example_scenario() {
        let ds = Dataset::load(&example_dir()).unwrap();
        assert_eq!(ds.meta.id, "example/github/basic");
        assert_eq!(ds.meta.clock_step_secs, LogicalClock::DEFAULT_STEP_SECS);
        let github = &ds.services["github"];
        let issues = github.collection("issues").unwrap();
        let keys: Vec<&str> = issues.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["octo-org/app#1", "octo-org/app#2", "octo-org/app#3"]);
        // `_key` is consumed, not served.
        assert!(issues.get("octo-org/app#1").unwrap().get("_key").is_none());
        assert!(github.collection("viewer").unwrap().contains("viewer"));
    }

    #[test]
    fn falls_back_to_id_as_key() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "scenario.toml",
            "[scenario]\nid = \"t\"\nnow = \"2026-01-01T00:00:00Z\"\nseed = \"t\"\nclock_step_secs = 3\n",
        );
        write(
            dir.path(),
            "dataset/stripe/customers.json",
            r#"[{"id":"cus_1","email":"a@example.com"},{"id":42}]"#,
        );
        let ds = Dataset::load(dir.path()).unwrap();
        assert_eq!(ds.meta.clock_step_secs, 3);
        let customers = ds.services["stripe"].collection("customers").unwrap();
        assert_eq!(
            customers.get("cus_1"),
            Some(&json!({"id": "cus_1", "email": "a@example.com"}))
        );
        assert!(customers.contains("42"));
    }

    #[test]
    fn rejects_documents_without_key() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "scenario.toml",
            "[scenario]\nid = \"t\"\nnow = \"2026-01-01T00:00:00Z\"\nseed = \"t\"\n",
        );
        write(
            dir.path(),
            "dataset/stripe/customers.json",
            r#"[{"email":"a@example.com"}]"#,
        );
        let err = Dataset::load(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("document 0 has no \"_key\" or \"id\""),
            "{err}"
        );
    }

    #[test]
    fn rejects_duplicate_keys() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "scenario.toml",
            "[scenario]\nid = \"t\"\nnow = \"2026-01-01T00:00:00Z\"\nseed = \"t\"\n",
        );
        write(
            dir.path(),
            "dataset/stripe/customers.json",
            r#"[{"id":"a"},{"id":"a"}]"#,
        );
        let err = Dataset::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("duplicate key \"a\""), "{err}");
    }

    #[test]
    fn rejects_non_array_files() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "scenario.toml",
            "[scenario]\nid = \"t\"\nnow = \"2026-01-01T00:00:00Z\"\nseed = \"t\"\n",
        );
        write(dir.path(), "dataset/stripe/customers.json", r#"{"id":"a"}"#);
        let err = Dataset::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("expected a JSON array"), "{err}");
    }

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}
