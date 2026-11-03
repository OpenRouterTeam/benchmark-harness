use anyhow::{Context, anyhow};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};

use crate::stdio::into_result;

/// Minimal streamable-HTTP MCP client: JSON or single-response SSE replies.
pub struct HttpClient {
    url: String,
    http: reqwest::Client,
    session: Option<String>,
    next_id: u64,
}

impl HttpClient {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("reqwest client"),
            session: None,
            next_id: 1,
        }
    }

    pub async fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let response = self.post(&msg).await?;
        if let Some(session) = response.headers().get("mcp-session-id") {
            self.session = Some(session.to_str()?.to_owned());
        }
        let status = response.status();
        let is_sse = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let body = response.text().await?;
        if !status.is_success() {
            return Err(anyhow!("{method}: HTTP {status}: {body}"));
        }
        let reply = if is_sse {
            sse_message(&body, id).with_context(|| format!("{method}: no response event"))?
        } else {
            serde_json::from_str(&body)?
        };
        into_result(reply)
    }

    pub async fn notify(&mut self, msg: Value) -> anyhow::Result<()> {
        let response = self.post(&msg).await?;
        if !response.status().is_success() {
            return Err(anyhow!("notification: HTTP {}", response.status()));
        }
        Ok(())
    }

    pub async fn close(self) {
        if let Some(session) = &self.session {
            let _ = self
                .http
                .delete(&self.url)
                .header("mcp-session-id", session)
                .send()
                .await;
        }
    }

    async fn post(&self, msg: &Value) -> anyhow::Result<reqwest::Response> {
        let mut request = self
            .http
            .post(&self.url)
            .header(ACCEPT, "application/json, text/event-stream")
            .json(msg);
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        Ok(request.send().await?)
    }
}

/// The JSON-RPC message with `id` among the `data:` events of an SSE body.
fn sse_message(body: &str, id: u64) -> Option<Value> {
    body.split("\n\n")
        .filter_map(|event| {
            let data: Vec<&str> = event
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(str::trim_start)
                .collect();
            serde_json::from_str::<Value>(&data.join("\n")).ok()
        })
        .find(|msg| msg.get("id") == Some(&json!(id)))
}
