//! Determinism check: replays a recorded journal against a fresh world started from the
//! same scenario and reports every response that differs from the recording.

use std::{
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

use mock_core::{
    dataset::Dataset,
    journal::{JournalEntry, body_value},
    world::{WorldState, shared},
};

use crate::{PortChoice, spawn};

#[derive(Debug, Clone, PartialEq)]
pub struct Mismatch {
    pub seq: u64,
    pub detail: String,
}

pub async fn replay(
    scenario_dir: &Path,
    entries: &[JournalEntry],
) -> anyhow::Result<Vec<Mismatch>> {
    let world = shared(WorldState::from_dataset(Dataset::load(scenario_dir)?)?);
    let running = spawn(
        world,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        PortChoice::Ephemeral,
    )
    .await?;
    let http = reqwest::Client::builder().no_proxy().build()?;
    let mut mismatches = Vec::new();
    for entry in entries {
        // Sub-requests recorded inside a tool call are reproduced by replaying that call.
        if entry.scheme == "internal" {
            continue;
        }
        let Some(addr) = running.services.get(entry.service.as_str()) else {
            mismatches.push(Mismatch {
                seq: entry.seq,
                detail: format!("unknown service {}", entry.service),
            });
            continue;
        };
        let url = match &entry.query {
            Some(q) => format!("http://{addr}{}?{q}", entry.path),
            None => format!("http://{addr}{}", entry.path),
        };
        let method = reqwest::Method::from_bytes(entry.method.as_bytes())?;
        let mut request = http.request(method, url);
        if let Some(host) = &entry.host {
            request = request.header("host", host);
        }
        if entry.scheme == "https" {
            request = request.header("x-forwarded-proto", "https");
        }
        if let Some(ct) = &entry.content_type {
            request = request.header("content-type", ct);
        }
        request = match &entry.request_body {
            serde_json::Value::Null => request,
            serde_json::Value::String(s) => request.body(s.clone()),
            other => request.body(other.to_string()),
        };
        let response = request.send().await?;
        let status = response.status().as_u16();
        let body = body_value(&response.bytes().await?);
        if status != entry.status {
            mismatches.push(Mismatch {
                seq: entry.seq,
                detail: format!("status {status} != recorded {}", entry.status),
            });
        } else if body != entry.response_body {
            mismatches.push(Mismatch {
                seq: entry.seq,
                detail: format!("{} {} body differs", entry.method, entry.path),
            });
        }
    }
    Ok(mismatches)
}
