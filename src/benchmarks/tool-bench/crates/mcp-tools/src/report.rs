//! Markdown summary of captured snapshots, with schema lint flags.

use serde_json::Value;

use crate::snapshot::Snapshot;

/// Schema properties worth flagging. Tools are never edited; this only reports.
pub fn lint_tool(tool: &Value) -> Vec<String> {
    let mut flags = Vec::new();
    let schema = &tool["inputSchema"];
    if schema.is_null() {
        flags.push("no inputSchema".to_owned());
        return flags;
    }
    if schema["type"] != "object" {
        flags.push("inputSchema.type is not \"object\"".to_owned());
    }
    if let Some(dialect) = schema["$schema"].as_str()
        && (dialect.contains("draft-04") || dialect.contains("draft-07"))
    {
        flags.push(format!("$schema {dialect}"));
    }
    if tool["description"].as_str().is_none_or(str::is_empty) {
        flags.push("no description".to_owned());
    }
    flags
}

pub fn render(snapshots: &[Snapshot]) -> String {
    let mut out = String::from(
        "| Endpoint | Server | Tools | Bytes | o200k tokens | Flags | sha256 |\n|---|---|---|---|---|---|---|\n",
    );
    let (mut tools, mut bytes, mut tokens) = (0, 0, 0);
    for snap in snapshots {
        let flagged: usize = snap
            .tools
            .iter()
            .filter(|t| !lint_tool(t).is_empty())
            .count();
        out.push_str(&format!(
            "| {} | {} {} | {} | {} | {} | {} | `{}` |\n",
            snap.name,
            snap.server_info["name"].as_str().unwrap_or("?"),
            snap.server_info["version"].as_str().unwrap_or("?"),
            snap.counts.tools,
            snap.counts.bytes,
            snap.counts.o200k,
            flagged,
            &snap.sha256[..12],
        ));
        tools += snap.counts.tools;
        bytes += snap.counts.bytes;
        tokens += snap.counts.o200k;
    }
    out.push_str(&format!(
        "| **Total ({} endpoints)** | | **{tools}** | **{bytes}** | **{tokens}** | | |\n",
        snapshots.len()
    ));
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn flags_old_dialects_and_missing_parts() {
        assert!(
            lint_tool(&json!({"name": "a", "description": "d", "inputSchema": {"type": "object"}}))
                .is_empty()
        );
        let flags = lint_tool(
            &json!({"name": "a", "inputSchema": {"$schema": "http://json-schema.org/draft-07/schema#", "type": "object"}}),
        );
        assert_eq!(
            flags,
            [
                "$schema http://json-schema.org/draft-07/schema#",
                "no description"
            ]
        );
        assert_eq!(
            lint_tool(&json!({"name": "a", "description": "d"})),
            ["no inputSchema"]
        );
    }

    #[test]
    fn totals_add_up() {
        let a = Snapshot::new(
            "a",
            json!({"name": "s", "version": "1"}),
            json!("2025-06-18"),
            vec![json!({"name": "x", "description": "d", "inputSchema": {"type": "object"}})],
        );
        let b = Snapshot::new(
            "b",
            json!({"name": "s", "version": "1"}),
            json!("2025-06-18"),
            vec![json!({"name": "y"})],
        );
        let md = render(&[a.clone(), b.clone()]);
        assert!(md.contains(&format!(
            "| **Total (2 endpoints)** | | **2** | **{}** |",
            a.counts.bytes + b.counts.bytes
        )));
        assert!(md.contains("| b | s 1 | 1 |"));
    }
}
