//! tools-supervisor [--catalog <toml>] [--snapshots <dir>] [--ca-url <url>] [--only <id,...>]
//!
//! Entry point of the tools container: serves every catalog endpoint on its MCP port.
//! Real endpoints run their stdio server behind the bridge (one child per session);
//! replica endpoints serve their snapshot. When `--ca-url` is given, the world's CA is
//! fetched first and every child is told to trust it.

use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

use anyhow::{Context, bail};
use catalog::{CONFIG_ROOT, Catalog, Runtime};
use mcp_tools::{bridge::StdioBackend, front, replica::ReplicaBackend, snapshot::Snapshot};
use tokio::{net::TcpListener, task::JoinSet};

/// Env vars that point each runtime's TLS stack at the combined CA bundle.
const TRUST_VARS: [&str; 4] = [
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
];
const SYSTEM_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut catalog_path = PathBuf::from(format!("{CONFIG_ROOT}/services.toml"));
    let mut snapshots = PathBuf::from("/opt/tool-bench/snapshots");
    let mut ca_url = None;
    let mut only: Option<Vec<String>> = None;
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--catalog" => catalog_path = PathBuf::from(value()?),
            "--snapshots" => snapshots = PathBuf::from(value()?),
            "--ca-url" => ca_url = Some(value()?),
            "--only" => only = Some(value()?.split(',').map(str::to_owned).collect()),
            other => bail!("unexpected argument {other:?}"),
        }
    }
    let catalog = Catalog::load(&catalog_path)?;
    catalog.validate()?;

    let trust = match ca_url {
        Some(url) => trust_env(&fetch_ca(&url).await?)?,
        None => BTreeMap::new(),
    };

    let mut tasks = JoinSet::new();
    for service in &catalog.services {
        for endpoint in &service.endpoints {
            if only.as_ref().is_some_and(|o| !o.contains(&endpoint.id)) {
                continue;
            }
            let listener = TcpListener::bind(("0.0.0.0", endpoint.mcp_port)).await?;
            let app = if endpoint.runtime == Runtime::Replica {
                let file = snapshots.join(format!("{}.json", endpoint.id));
                let snapshot: Snapshot = serde_json::from_str(
                    &fs::read_to_string(&file)
                        .with_context(|| format!("reading {}", file.display()))?,
                )?;
                let forward = format!(
                    "http://{}:{}{}",
                    catalog::WORLD_HOST,
                    service.world_port,
                    mcp_tools::replica::TOOLS_CALL_PATH
                );
                front::router(ReplicaBackend::new(snapshot).forward_to(forward))
            } else {
                let mut spec = catalog.launch(&endpoint.id)?;
                spec.env.extend(trust.clone());
                front::router(StdioBackend::new(spec).sort_tools(endpoint.sort_tools))
            };
            eprintln!(
                "tools: {} ({}) on :{}",
                endpoint.id,
                endpoint.runtime.as_str(),
                endpoint.mcp_port
            );
            tasks.spawn(async move { axum::serve(listener, app).await });
        }
    }
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

/// Polls the world admin API until it publishes its CA (the world may still be starting).
async fn fetch_ca(url: &str) -> anyhow::Result<String> {
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    for _ in 0..120 {
        if let Ok(resp) = http.get(url).send().await
            && resp.status().is_success()
        {
            return Ok(resp.text().await?);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("world CA not available at {url}")
}

/// Writes system roots + world CA to one bundle and returns the env pointing at it.
fn trust_env(ca_pem: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let dir = PathBuf::from(CONFIG_ROOT);
    fs::create_dir_all(&dir)?;
    let ca_path = dir.join("world-ca.pem");
    fs::write(&ca_path, ca_pem)?;
    let bundle_path = dir.join("ca-bundle.pem");
    let system = fs::read_to_string(SYSTEM_BUNDLE).unwrap_or_default();
    fs::write(&bundle_path, format!("{system}\n{ca_pem}"))?;
    Ok(TRUST_VARS
        .iter()
        .map(|var| {
            let path = if *var == "NODE_EXTRA_CA_CERTS" {
                &ca_path
            } else {
                &bundle_path
            };
            ((*var).to_owned(), path.display().to_string())
        })
        .collect())
}
