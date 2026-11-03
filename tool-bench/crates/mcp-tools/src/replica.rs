//! Serves a captured tool list verbatim, for services whose real MCP server cannot be
//! pointed at the world. Tool calls are answered by per-tool handlers (added with the
//! mock APIs in P4); a tool without a handler returns an MCP tool error.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::{
    PROTOCOL_VERSION,
    front::{Backend, Reply},
    snapshot::Snapshot,
};

pub struct ReplicaBackend {
    snapshot: Snapshot,
    /// Where `tools/call` is forwarded (the service's `/_mcp/tools/call` in the world).
    forward: Option<String>,
    http: reqwest::Client,
}

/// Path every replica service serves on its world port for tool calls.
pub const TOOLS_CALL_PATH: &str = "/_mcp/tools/call";

impl ReplicaBackend {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot,
            forward: None,
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("reqwest client"),
        }
    }

    /// Forward tool calls to `url`, which answers with an MCP `CallToolResult`.
    pub fn forward_to(mut self, url: impl Into<String>) -> Self {
        self.forward = Some(url.into());
        self
    }

    async fn call_tool(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params["name"].as_str().unwrap_or_default();
        if !self.snapshot.tools.iter().any(|t| t["name"] == name) {
            return Err((-32602, format!("Unknown tool: {name}")));
        }
        let unavailable = || {
            json!({
                "content": [{ "type": "text", "text": format!("{name} is not available in this scenario") }],
                "isError": true,
            })
        };
        let Some(url) = &self.forward else {
            return Ok(unavailable());
        };
        let body = json!({ "name": name, "arguments": params.get("arguments").cloned().unwrap_or(json!({})) });
        match self.http.post(url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => resp
                .json::<Value>()
                .await
                .map_err(|e| (-32603, format!("bad tool result from world: {e}"))),
            Ok(_) => Ok(unavailable()),
            Err(e) => Err((-32603, format!("world unreachable: {e}"))),
        }
    }

    fn handle(&self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": self.snapshot.server_info["name"],
                    "version": self.snapshot.server_info["version"],
                },
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.snapshot.tools })),
            _ => Err((-32601, "Method not found".to_owned())),
        }
    }
}

impl Backend for ReplicaBackend {
    type Session = ();

    async fn open(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn request(&self, _session: &(), msg: Value) -> anyhow::Result<Reply> {
        let id = msg["id"].clone();
        let method = msg["method"].as_str().unwrap_or_default();
        let outcome = if method == "tools/call" {
            self.call_tool(&msg["params"]).await
        } else {
            self.handle(method, &msg["params"])
        };
        Ok(Reply::Json(match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        }))
    }

    async fn notify(&self, _session: &(), _msg: Value) -> anyhow::Result<()> {
        Ok(())
    }

    async fn close(&self, _session: Arc<()>) {}
}
