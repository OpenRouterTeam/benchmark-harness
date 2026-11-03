//! `mcp-bridge --listen 0.0.0.0:7101 [--env KEY=VALUE]... -- <command> [args...]`
//!
//! Serves one stdio MCP server over streamable HTTP at `/mcp`.

use std::collections::BTreeMap;

use anyhow::{Context, bail};
use mcp_tools::{bridge::StdioBackend, front, launch::LaunchSpec};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut listen = None;
    let mut env = BTreeMap::new();
    let mut command = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = Some(args.next().context("--listen needs an address")?),
            "--env" => {
                let pair = args.next().context("--env needs KEY=VALUE")?;
                let (k, v) = pair.split_once('=').context("--env needs KEY=VALUE")?;
                env.insert(k.to_owned(), v.to_owned());
            }
            "--" => {
                command.extend(args.by_ref());
            }
            other => bail!("unexpected argument {other:?}"),
        }
    }
    let listen = listen.context("--listen is required")?;
    let (program, rest) = command.split_first().context("missing -- <command>")?;
    let spec = LaunchSpec {
        command: program.clone(),
        args: rest.to_vec(),
        env,
    };
    let listener = TcpListener::bind(&listen).await?;
    eprintln!("mcp-bridge: {} on {listen}", spec.command);
    axum::serve(listener, front::router(StdioBackend::new(spec))).await?;
    Ok(())
}
