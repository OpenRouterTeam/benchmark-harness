//! Serves a stdio MCP server over streamable HTTP: one child process per MCP session.

use std::sync::Arc;

use serde_json::Value;

use crate::{
    front::{Backend, Reply},
    launch::LaunchSpec,
    stdio::StdioConn,
};

pub struct StdioBackend {
    spec: LaunchSpec,
    sort_tools: bool,
}

impl StdioBackend {
    pub fn new(spec: LaunchSpec) -> Self {
        Self {
            spec,
            sort_tools: false,
        }
    }

    /// Return `tools/list` sorted by tool name, for servers whose listing order is random.
    pub fn sort_tools(mut self, sort: bool) -> Self {
        self.sort_tools = sort;
        self
    }
}

/// Sorts the `tools` array of a `tools/list` response by name, leaving each tool untouched.
pub fn sort_tools_by_name(response: &mut Value) {
    if let Some(tools) = response
        .get_mut("result")
        .and_then(|r| r.get_mut("tools"))
        .and_then(Value::as_array_mut)
    {
        tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    }
}

impl Backend for StdioBackend {
    type Session = Arc<StdioConn>;

    async fn open(&self) -> anyhow::Result<Self::Session> {
        StdioConn::spawn(&self.spec).await
    }

    async fn request(&self, session: &Self::Session, msg: Value) -> anyhow::Result<Reply> {
        // Tool calls stream, so elicitation/sampling/roots requests reach the client.
        if msg["method"] == "tools/call" {
            return Ok(Reply::Stream(session.call_streaming(msg)));
        }
        let is_list = msg["method"] == "tools/list";
        let mut response = session.call(msg).await?;
        if self.sort_tools && is_list {
            sort_tools_by_name(&mut response);
        }
        Ok(Reply::Json(response))
    }

    async fn notify(&self, session: &Self::Session, msg: Value) -> anyhow::Result<()> {
        session.notify(msg).await
    }

    async fn close(&self, session: Arc<Self::Session>) {
        session.shutdown().await;
    }
}
