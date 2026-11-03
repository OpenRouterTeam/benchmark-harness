//! taskgen [--scenarios scenarios] [--catalog catalog/services.toml] [--out tasks]
//!         [--world-image IMG] [--tools-image IMG] [--check] [--print-images]
//!
//! Validates every scenario and regenerates its Harbor task. The images default to the
//! published, content-tagged references (`scenario::images`). `--check` fails if `--out`
//! differs from what would be generated (CI guard for hand edits and unpublished sources).
//! `--print-images` prints the two references (the defaults unless overridden) and exits.
//! Sources are hashed only for a reference that is not overridden.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use catalog::Catalog;
use scenario::{
    discover, images,
    taskgen::{Images, write_task},
};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut scenarios = PathBuf::from("scenarios");
    let mut catalog_path = PathBuf::from("catalog/services.toml");
    let mut out = PathBuf::from("tasks");
    let mut world_image = None;
    let mut tools_image = None;
    let mut print_images = false;
    let mut check = false;
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--scenarios" => scenarios = PathBuf::from(value()?),
            "--catalog" => catalog_path = PathBuf::from(value()?),
            "--out" => out = PathBuf::from(value()?),
            "--world-image" => world_image = Some(value()?),
            "--tools-image" => tools_image = Some(value()?),
            "--check" => check = true,
            "--print-images" => print_images = true,
            other => bail!("unexpected argument {other:?}"),
        }
    }
    // Hash the image sources only for a reference that wasn't given.
    let root = Path::new(".");
    let images = Images {
        world: world_image.map_or_else(|| images::world(root), Ok)?,
        tools: tools_image.map_or_else(|| images::tools(root), Ok)?,
    };
    if print_images {
        println!("{}\n{}", images.world, images.tools);
        return Ok(());
    }
    let catalog = Catalog::load(&catalog_path)?;
    catalog.validate()?;
    let all = discover(&scenarios)?;
    for s in &all {
        s.validate(&scenarios, &catalog)?;
    }
    // Example scenarios exercise the mocks but are not benchmark tasks.
    let tasks: Vec<_> = all
        .iter()
        .filter(|s| !s.meta.id.starts_with("example/"))
        .collect();

    let target = if check {
        std::env::temp_dir().join("tool-bench-taskgen-check")
    } else {
        out.clone()
    };
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::create_dir_all(&target)?;
    for s in &tasks {
        write_task(s, &catalog, &images, &target)?;
    }
    if check {
        let status = std::process::Command::new("diff")
            .arg("-r")
            .arg(&out)
            .arg(&target)
            .status()?;
        if !status.success() {
            bail!("{} is stale; run taskgen", out.display());
        }
        println!("ok: {} tasks up to date", tasks.len());
    } else {
        println!(
            "wrote {} tasks to {} ({} scenarios validated)",
            tasks.len(),
            out.display(),
            all.len()
        );
    }
    Ok(())
}
