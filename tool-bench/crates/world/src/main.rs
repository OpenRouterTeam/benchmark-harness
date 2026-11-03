use std::{
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
};

use anyhow::Context;
use mock_core::{
    dataset::Dataset,
    world::{WorldState, shared},
};
use world::{PortChoice, spawn};

const SCENARIO_ROOT: &str = "/opt/tool-bench/scenarios";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // SCENARIO_DIR wins; otherwise SCENARIO_ID under the image's scenario root.
    let dir = match (
        std::env::var_os("SCENARIO_DIR"),
        std::env::var("SCENARIO_ID"),
    ) {
        (Some(dir), _) => PathBuf::from(dir),
        (None, Ok(id)) => PathBuf::from(SCENARIO_ROOT).join(id),
        (None, Err(_)) => anyhow::bail!("set SCENARIO_DIR or SCENARIO_ID"),
    };
    let dataset =
        Dataset::load(&dir).with_context(|| format!("loading scenario {}", dir.display()))?;
    let scenario = dataset.meta.id.clone();
    let world = shared(WorldState::from_dataset(dataset)?);

    let running = spawn(world, IpAddr::V4(Ipv4Addr::UNSPECIFIED), PortChoice::Fixed).await?;
    eprintln!("world: scenario {scenario}, admin on {}", running.admin);
    for (name, addr) in &running.services {
        eprintln!("world: {name} on {addr}");
    }
    running.wait().await?;
    Ok(())
}
