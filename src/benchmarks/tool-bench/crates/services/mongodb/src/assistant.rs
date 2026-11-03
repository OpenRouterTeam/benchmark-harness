//! MongoDB Assistant knowledge-base tools, answered from a small pinned corpus
//! (plus any `knowledge.json` entries in the dataset). Output is YAML, as the server
//! renders it with `yaml.stringify`.

use serde_json::{Value, json};

use crate::{Ctx, ok, str_arg};

fn sources() -> Value {
    json!([
        { "id": "docs", "type": "tech-docs", "versions": [ { "label": "8.0", "isCurrent": true }, { "label": "7.0", "isCurrent": false } ] },
        { "id": "atlas", "type": "tech-docs", "versions": [ { "label": "current", "isCurrent": true } ] },
        { "id": "node", "type": "tech-docs", "versions": [ { "label": "6.x", "isCurrent": true } ] },
        { "id": "mcp-server", "type": "tech-docs", "versions": [ { "label": "current", "isCurrent": true } ] }
    ])
}

fn corpus() -> Vec<Value> {
    let e = |url: &str, title: &str, source: &str, text: &str| json!({ "url": url, "title": title, "text": text, "metadata": { "sourceName": source, "tags": [] } });
    vec![
        e(
            "https://www.mongodb.com/docs/manual/core/aggregation-pipeline/",
            "Aggregation Pipeline",
            "docs",
            "An aggregation pipeline consists of one or more stages that process documents. Each stage performs an operation on the input documents, for example $match filters documents, $group groups documents by a key and computes accumulators such as $sum and $avg, and $sort reorders them.",
        ),
        e(
            "https://www.mongodb.com/docs/manual/reference/operator/aggregation/group/",
            "$group (aggregation)",
            "docs",
            "The $group stage separates documents into groups according to a group key. The output is one document for each unique group key. Accumulators include $sum, $avg, $min, $max, $first, $last, $push and $addToSet.",
        ),
        e(
            "https://www.mongodb.com/docs/manual/reference/operator/aggregation/lookup/",
            "$lookup (aggregation)",
            "docs",
            "Performs a left outer join to a collection in the same database to filter in documents from the joined collection for processing. Specify from, localField, foreignField and as, or a pipeline with let variables.",
        ),
        e(
            "https://www.mongodb.com/docs/manual/indexes/",
            "Indexes",
            "docs",
            "Indexes support efficient execution of queries. Without indexes, MongoDB must scan every document in a collection (a COLLSCAN). Create compound indexes following the ESR (Equality, Sort, Range) guideline.",
        ),
        e(
            "https://www.mongodb.com/docs/manual/reference/method/db.collection.updateMany/",
            "db.collection.updateMany()",
            "docs",
            "Updates all documents that match the specified filter for a collection. The update document must contain update operator expressions such as $set, $unset, $inc or $push, or be an aggregation pipeline.",
        ),
        e(
            "https://www.mongodb.com/docs/manual/reference/method/db.collection.deleteMany/",
            "db.collection.deleteMany()",
            "docs",
            "Removes all documents that match the filter from a collection. To delete all documents, pass an empty filter document {}. deleteMany cannot be used on a capped collection.",
        ),
        e(
            "https://www.mongodb.com/docs/manual/core/schema-validation/",
            "Schema Validation",
            "docs",
            "Schema validation lets you create validation rules for your fields, such as allowed data types and value ranges, using $jsonSchema.",
        ),
        e(
            "https://www.mongodb.com/docs/atlas/security/ip-access-list/",
            "Configure IP Access List Entries",
            "atlas",
            "Atlas only allows client connections to a cluster from entries in the project's IP access list. Each entry is either a single IP address or a CIDR-notated range of addresses.",
        ),
        e(
            "https://www.mongodb.com/docs/atlas/pause-terminate-cluster/",
            "Pause, Resume, or Terminate a Cluster",
            "atlas",
            "You can pause an M10+ dedicated cluster for up to 30 days. Atlas does not charge compute costs for paused clusters, but storage charges still apply. You cannot pause M0 free clusters or Flex clusters.",
        ),
        e(
            "https://www.mongodb.com/docs/atlas/performance-advisor/",
            "Monitor and Improve Slow Queries",
            "atlas",
            "The Performance Advisor monitors queries that MongoDB considers slow and suggests new indexes to improve query performance, as well as indexes to drop because they are unused or redundant.",
        ),
        e(
            "https://www.mongodb.com/docs/atlas/security-add-mongodb-users/",
            "Configure Database Users",
            "atlas",
            "Database users authenticate to clusters. Assign built-in roles such as read, readWrite or atlasAdmin, or custom roles, and optionally restrict users to specific clusters.",
        ),
        e(
            "https://www.mongodb.com/docs/mcp-server/tools/",
            "MongoDB MCP Server Tools",
            "mcp-server",
            "The MongoDB MCP Server exposes database tools (find, aggregate, insert-many, update-many, ...) and Atlas tools (atlas-list-projects, atlas-list-clusters, atlas-connect-cluster, ...). Database tools take a connectionId returned by connect or atlas-connect-cluster.",
        ),
    ]
}

/// Minimal YAML rendering for the knowledge-base payloads (arrays of flat-ish objects).
fn yaml(v: &Value, indent: usize) -> String {
    let pad = " ".repeat(indent);
    match v {
        Value::Array(items) if items.is_empty() => "[]\n".to_owned(),
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                match item {
                    Value::Object(_) => {
                        let body = yaml(item, indent + 2);
                        let body = body.trim_start_matches(' ');
                        out.push_str(&format!("{pad}- {body}"));
                    }
                    other => out.push_str(&format!("{pad}- {}\n", scalar(other))),
                }
            }
            out
        }
        Value::Object(m) if m.is_empty() => "{}\n".to_owned(),
        Value::Object(m) => {
            let mut out = String::new();
            for (k, val) in m {
                match val {
                    Value::Array(a) if !a.is_empty() => {
                        out.push_str(&format!("{pad}{k}:\n{}", yaml(val, indent + 2)));
                    }
                    Value::Object(o) if !o.is_empty() => {
                        out.push_str(&format!("{pad}{k}:\n{}", yaml(val, indent + 2)));
                    }
                    Value::Array(_) => out.push_str(&format!("{pad}{k}: []\n")),
                    Value::Object(_) => out.push_str(&format!("{pad}{k}: {{}}\n")),
                    other => out.push_str(&format!("{pad}{k}: {}\n", scalar(other))),
                }
            }
            out
        }
        other => format!("{pad}{}\n", scalar(other)),
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => {
            let plain = !s.is_empty()
                && !s.contains(": ")
                && !s.contains(" #")
                && !s.starts_with(|c: char| "-?:,[]{}#&*!|>'\"%@`".contains(c))
                && s.parse::<f64>().is_err()
                && !matches!(s.as_str(), "true" | "false" | "null" | "yes" | "no");
            if plain {
                s.clone()
            } else {
                serde_json::to_string(s).expect("string serializes")
            }
        }
        Value::Null => "null".to_owned(),
        other => other.to_string(),
    }
}

pub fn call(ctx: &mut Ctx, name: &str, args: &Value) -> Value {
    if name == "list-knowledge-sources" {
        let list: Vec<Value> = sources()
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut ds| {
                if let Some(cur) = ds["versions"]
                    .as_array()
                    .and_then(|v| v.iter().find(|x| x["isCurrent"] == true))
                    .map(|x| x["label"].clone())
                {
                    ds["currentVersion"] = cur;
                }
                ds
            })
            .collect();
        let n = list.len();
        let content = ctx.untrusted(
            &format!("Found {n} data sources in the MongoDB Assistant knowledge base."),
            vec![yaml(&Value::Array(list), 0)],
        );
        return ok(content, None);
    }
    let q = str_arg(args, "query").unwrap_or("").to_lowercase();
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(1, 100) as usize;
    let filter: Option<Vec<String>> = args["dataSources"].as_array().map(|a| {
        a.iter()
            .filter_map(|d| d["name"].as_str().map(str::to_owned))
            .collect()
    });
    let mut entries = ctx.docs("knowledge");
    entries.extend(corpus());
    let words: Vec<&str> = q
        .split(|c: char| !c.is_alphanumeric() && c != '$')
        .filter(|w| w.len() > 2)
        .collect();
    let mut scored: Vec<(usize, usize, Value)> = entries
        .into_iter()
        .enumerate()
        .filter(|(_, e)| {
            filter.as_ref().is_none_or(|f| {
                e["metadata"]["sourceName"]
                    .as_str()
                    .is_some_and(|s| f.iter().any(|x| x == s))
            })
        })
        .map(|(i, e)| {
            let hay = format!(
                "{} {}",
                e["title"].as_str().unwrap_or(""),
                e["text"].as_str().unwrap_or("")
            )
            .to_lowercase();
            (words.iter().filter(|w| hay.contains(*w)).count(), i, e)
        })
        .filter(|(score, _, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let results: Vec<Value> = scored.into_iter().take(limit).map(|(_, _, e)| e).collect();
    let n = results.len();
    let content = ctx.untrusted(
        &format!("Found {n} results in the MongoDB Assistant knowledge base."),
        vec![yaml(&Value::Array(results), 0)],
    );
    ok(content, None)
}
