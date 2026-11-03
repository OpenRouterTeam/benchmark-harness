//! Scripted stdio MCP server used by this crate's tests.
//!
//! - Prints one non-JSON line first (some real servers log to stdout).
//! - `tools/list` pages through `FAKE_TOOL_PAGES` pages (default 1) of two tools each.
//! - Tool `echo` returns its arguments; `env` returns the process environment keys;
//!   `roots` asks the client for `roots/list` and returns whatever came back.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    writeln!(out, "fake-mcp-server starting").unwrap();
    out.flush().unwrap();

    let pages: usize = std::env::var("FAKE_TOOL_PAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut lines = stdin.lock().lines();
    while let Some(Ok(line)) = lines.next() {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            continue; // notification
        };
        let result = match msg["method"].as_str().unwrap_or_default() {
            "initialize" => json!({
                "protocolVersion": msg["params"]["protocolVersion"],
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fake", "version": "1.0.0" }
            }),
            "tools/list" => {
                let page: usize = msg["params"]["cursor"]
                    .as_str()
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                let tools: Vec<Value> = (0..2)
                    .map(|i| {
                        json!({
                            "name": format!("tool_{page}_{i}"),
                            "description": "A fake tool.",
                            "inputSchema": { "type": "object", "properties": {} }
                        })
                    })
                    .collect();
                if page + 1 < pages {
                    json!({ "tools": tools, "nextCursor": (page + 1).to_string() })
                } else {
                    json!({ "tools": tools })
                }
            }
            "tools/call" => match msg["params"]["name"].as_str().unwrap_or_default() {
                "echo" => text(&msg["params"]["arguments"].to_string()),
                "env" => {
                    let mut keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
                    keys.sort();
                    text(&keys.join(","))
                }
                "roots" => {
                    let request = json!({"jsonrpc": "2.0", "id": "srv-1", "method": "roots/list"});
                    writeln!(out, "{request}").unwrap();
                    out.flush().unwrap();
                    let reply = lines.next().and_then(|l| l.ok()).unwrap_or_default();
                    text(&reply)
                }
                other => {
                    let error = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("unknown tool {other}")}});
                    writeln!(out, "{error}").unwrap();
                    out.flush().unwrap();
                    continue;
                }
            },
            _ => {
                let error = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}});
                writeln!(out, "{error}").unwrap();
                out.flush().unwrap();
                continue;
            }
        };
        writeln!(
            out,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        )
        .unwrap();
        out.flush().unwrap();
    }
}

fn text(s: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": s }] })
}
