//! Captured `tools/list` output with a content hash and size counts.

use std::sync::OnceLock;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{PROTOCOL_VERSION, client::McpClient};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub tools: usize,
    /// Bytes of the compact JSON of `tools`.
    pub bytes: usize,
    /// o200k_base tokens of the compact JSON of `tools` (offline, provider-neutral estimate).
    pub o200k: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Catalog endpoint id, e.g. `github`.
    pub name: String,
    /// `serverInfo` from `initialize`.
    pub server_info: Value,
    pub protocol_version: Value,
    /// Tools exactly as the server listed them, in order, across all pages.
    pub tools: Vec<Value>,
    /// SHA-256 of the compact JSON of `tools`.
    pub sha256: String,
    pub counts: Counts,
}

const MAX_PAGES: usize = 1000;

/// Initializes, lists every tool page, and closes the client.
pub async fn capture(client: &mut McpClient, name: &str) -> anyhow::Result<Snapshot> {
    let init = client
        .request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "tool-bench-toolmeter", "version": env!("CARGO_PKG_VERSION")}
            }),
        )
        .await
        .context("initialize")?;
    client
        .notify("notifications/initialized", json!({}))
        .await
        .context("notifications/initialized")?;

    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let params = match &cursor {
            Some(c) => json!({ "cursor": c }),
            None => json!({}),
        };
        let page = client
            .request("tools/list", params)
            .await
            .context("tools/list")?;
        if let Some(items) = page["tools"].as_array() {
            tools.extend(items.iter().cloned());
        }
        cursor = page["nextCursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(Snapshot::new(
        name,
        init["serverInfo"].clone(),
        init["protocolVersion"].clone(),
        tools,
    ))
}

impl Snapshot {
    pub fn new(name: &str, server_info: Value, protocol_version: Value, tools: Vec<Value>) -> Self {
        let bytes = serde_json::to_vec(&tools).expect("tools serialize");
        let text = String::from_utf8_lossy(&bytes);
        Self {
            name: name.to_owned(),
            server_info,
            protocol_version,
            counts: Counts {
                tools: tools.len(),
                bytes: bytes.len(),
                o200k: o200k_count(&text),
            },
            sha256: sha256_hex(&bytes),
            tools,
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn o200k_count(text: &str) -> usize {
    static BPE: OnceLock<tiktoken_rs::CoreBPE> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::o200k_base().expect("o200k_base tables"))
        .encode_with_special_tokens(text)
        .len()
}
