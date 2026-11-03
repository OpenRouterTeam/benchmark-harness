//! Helpers for services that implement replica tools on `/_mcp/tools/call`.
//! The body is `{"name": <tool>, "arguments": {...}}`; the reply is an MCP `CallToolResult`.

use serde::Deserialize;
use serde_json::{Value, json};

pub use crate::http::json as json_response;

pub const TOOLS_CALL_PATH: &str = "/_mcp/tools/call";

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// A successful result with one text block (most servers return JSON as text).
pub fn text(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}

/// A successful result whose text is `value` pretty-printed, as many servers do.
pub fn json_text(value: &Value) -> Value {
    text(serde_json::to_string_pretty(value).expect("JSON value serializes"))
}

/// A tool-level error (`isError: true`), which the model sees as the tool's output.
pub fn error(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": true })
}
