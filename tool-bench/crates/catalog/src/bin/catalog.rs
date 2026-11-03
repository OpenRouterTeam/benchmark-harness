//! catalog [--file catalog/services.toml] <command>
//!   validate                 check the catalog
//!   endpoints [runtime]      "<id>\t<runtime>\t<package>\t<version>" per endpoint
//!   launch <id>              JSON LaunchSpec for one endpoint
//!   aliases                  TLS hostnames the world container must answer for
//!   command <id>             the endpoint's executable name
//!   port <id>                the endpoint's MCP port in the tools container
//!   capture <id>|--all [--out dir]  run real endpoints over stdio and snapshot tools/list

use std::{fs, path::PathBuf};

use anyhow::{Context, bail};
use catalog::{Catalog, Runtime};
use mcp_tools::{client::McpClient, snapshot::capture, stdio::StdioConn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut file = PathBuf::from("catalog/services.toml");
    if args.first().map(String::as_str) == Some("--file") {
        args.remove(0);
        file = PathBuf::from(args.remove(0));
    }
    let catalog = Catalog::load(&file)?;
    match args.first().map(String::as_str) {
        Some("validate") => {
            catalog.validate()?;
            println!("ok: {} services", catalog.services.len());
        }
        Some("endpoints") => {
            let filter = args.get(1).map(String::as_str);
            for s in &catalog.services {
                for e in &s.endpoints {
                    let runtime = e.runtime.as_str();
                    if filter.is_none_or(|f| f == runtime) {
                        println!(
                            "{}\t{}\t{}\t{}",
                            e.id,
                            runtime,
                            e.package.as_deref().unwrap_or("-"),
                            e.version.as_deref().unwrap_or("-")
                        );
                    }
                }
            }
        }
        Some("launch") => {
            let id = args.get(1).context("launch <id>")?;
            println!("{}", serde_json::to_string(&catalog.launch(id)?)?);
        }
        Some("port") => {
            let id = args.get(1).context("port <id>")?;
            let (_, endpoint) = catalog
                .endpoint(id)
                .with_context(|| format!("no endpoint {id}"))?;
            println!("{}", endpoint.mcp_port);
        }
        Some("command") => {
            let id = args.get(1).context("command <id>")?;
            println!("{}", catalog.launch(id)?.command);
        }
        Some("capture") => {
            let target = args.get(1).context("capture <id>|--all")?.clone();
            let out = match args.iter().position(|a| a == "--out") {
                Some(i) => PathBuf::from(args.get(i + 1).context("--out needs a path")?),
                None => PathBuf::from("servers/snapshots"),
            };
            fs::create_dir_all(&out)?;
            let ids: Vec<String> = catalog
                .services
                .iter()
                .flat_map(|s| s.endpoints.iter())
                .filter(|e| e.runtime != Runtime::Replica)
                .filter(|e| target == "--all" || e.id == target)
                .map(|e| e.id.clone())
                .collect();
            let mut failures = 0;
            for id in ids {
                let result = async {
                    let mut client =
                        McpClient::Stdio(StdioConn::spawn(&catalog.launch(&id)?).await?);
                    let snapshot = capture(&mut client, &id).await;
                    client.close().await;
                    let mut snapshot = snapshot?;
                    if catalog.endpoint(&id).is_some_and(|(_, e)| e.sort_tools) {
                        snapshot
                            .tools
                            .sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                        snapshot = mcp_tools::snapshot::Snapshot::new(
                            &id,
                            snapshot.server_info,
                            snapshot.protocol_version,
                            snapshot.tools,
                        );
                    }
                    anyhow::Ok(snapshot)
                }
                .await;
                match result {
                    Ok(snapshot) => {
                        let path = out.join(format!("{id}.json"));
                        fs::write(&path, serde_json::to_string_pretty(&snapshot)? + "\n")?;
                        println!(
                            "{id}: {} tools, {} o200k tokens",
                            snapshot.counts.tools, snapshot.counts.o200k
                        );
                    }
                    Err(e) => {
                        failures += 1;
                        println!("{id}: FAILED {e:#}");
                    }
                }
            }
            if failures > 0 {
                bail!("{failures} endpoint(s) failed to capture");
            }
        }
        Some("aliases") => {
            for host in catalog.world_aliases() {
                println!("{host}");
            }
        }
        _ => bail!("usage: catalog [--file path] validate|endpoints [runtime]|launch <id>|aliases"),
    }
    Ok(())
}
