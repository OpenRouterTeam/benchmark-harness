//! toolmeter capture --name <endpoint> [--out <dir>] (--http <url> | [--env K=V]... -- <cmd> [args...])
//! toolmeter report [--dir <dir>]
//! toolmeter import --name <endpoint> --server <name> --version <v> --source <text> [--out <dir>] <tools.json>
//!   wraps an already-captured MCP tools array (replica sources) in a snapshot
//!
//! `capture` writes `<dir>/<endpoint>.json` (default dir `servers/snapshots`).

use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{Context, bail};
use mcp_tools::{
    client::McpClient,
    http_client::HttpClient,
    launch::LaunchSpec,
    report,
    snapshot::{Snapshot, capture},
    stdio::StdioConn,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let sub = args.next().context("usage: toolmeter capture|report ...")?;
    let mut name = None;
    let mut dir = PathBuf::from("servers/snapshots");
    let mut http = None;
    let mut env = BTreeMap::new();
    let mut command = Vec::new();
    let mut server = None;
    let mut version = None;
    let mut source = None;
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => name = Some(args.next().context("--name needs a value")?),
            "--out" | "--dir" => dir = PathBuf::from(args.next().context("--out needs a path")?),
            "--http" => http = Some(args.next().context("--http needs a URL")?),
            "--env" => {
                let pair = args.next().context("--env needs KEY=VALUE")?;
                let (k, v) = pair.split_once('=').context("--env needs KEY=VALUE")?;
                env.insert(k.to_owned(), v.to_owned());
            }
            "--server" => server = Some(args.next().context("--server needs a value")?),
            "--version" => version = Some(args.next().context("--version needs a value")?),
            "--source" => source = Some(args.next().context("--source needs a value")?),
            "--" => command.extend(args.by_ref()),
            other if !other.starts_with("--") => positional.push(other.to_owned()),
            other => bail!("unexpected argument {other:?}"),
        }
    }

    match sub.as_str() {
        "capture" => {
            let name = name.context("--name is required")?;
            let mut client = match (http, command.split_first()) {
                (Some(url), _) => McpClient::Http(HttpClient::new(url)),
                (None, Some((program, rest))) => McpClient::Stdio(
                    StdioConn::spawn(&LaunchSpec {
                        command: program.clone(),
                        args: rest.to_vec(),
                        env,
                    })
                    .await?,
                ),
                (None, None) => bail!("give --http <url> or -- <command>"),
            };
            let snapshot = capture(&mut client, &name).await;
            client.close().await;
            let snapshot = snapshot?;
            fs::create_dir_all(&dir)?;
            let path = dir.join(format!("{name}.json"));
            fs::write(&path, serde_json::to_string_pretty(&snapshot)? + "\n")?;
            println!(
                "{name}: {} tools, {} bytes, {} o200k tokens -> {}",
                snapshot.counts.tools,
                snapshot.counts.bytes,
                snapshot.counts.o200k,
                path.display()
            );
        }
        "import" => {
            let name = name.context("--name is required")?;
            let file = positional
                .first()
                .context("import needs a tools.json path")?;
            let tools: Vec<serde_json::Value> = serde_json::from_str(&fs::read_to_string(file)?)?;
            let info = serde_json::json!({
                "name": server.context("--server is required")?,
                "version": version.context("--version is required")?,
                "source": source.context("--source is required")?,
            });
            let snapshot = Snapshot::new(&name, info, serde_json::Value::Null, tools);
            fs::create_dir_all(&dir)?;
            let path = dir.join(format!("{name}.json"));
            fs::write(&path, serde_json::to_string_pretty(&snapshot)? + "\n")?;
            println!(
                "{name}: {} tools, {} o200k tokens -> {}",
                snapshot.counts.tools,
                snapshot.counts.o200k,
                path.display()
            );
        }
        "report" => {
            let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
                .with_context(|| format!("reading {}", dir.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            paths.sort();
            let snapshots = paths
                .iter()
                .map(|p| Ok(serde_json::from_str::<Snapshot>(&fs::read_to_string(p)?)?))
                .collect::<anyhow::Result<Vec<_>>>()?;
            print!("{}", report::render(&snapshots));
        }
        other => bail!("unknown subcommand {other:?}"),
    }
    Ok(())
}
