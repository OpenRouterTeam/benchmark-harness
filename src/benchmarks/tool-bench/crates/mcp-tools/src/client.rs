use std::sync::Arc;

use serde_json::{Value, json};

use crate::{http_client::HttpClient, stdio::StdioConn};

/// An MCP client over either transport.
pub enum McpClient {
    Stdio(Arc<StdioConn>),
    Http(HttpClient),
}

impl McpClient {
    /// Sends a request and returns its `result`; a JSON-RPC error becomes `Err`.
    pub async fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        match self {
            Self::Stdio(conn) => conn.request(method, params).await,
            Self::Http(http) => http.request(method, params).await,
        }
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        match self {
            Self::Stdio(conn) => conn.notify(msg).await,
            Self::Http(http) => http.notify(msg).await,
        }
    }

    pub async fn close(self) {
        match self {
            Self::Stdio(conn) => conn.shutdown().await,
            Self::Http(http) => http.close().await,
        }
    }
}
