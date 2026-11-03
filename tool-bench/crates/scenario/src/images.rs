//! Registry references for the world and tools images. Each tag is a hash of the files the
//! image is built from, so a committed task always names the image built from the same
//! sources; `taskgen --check` fails when those sources change without regenerating `tasks/`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use sha2::{Digest, Sha256};

use crate::taskgen::Images;

pub const REGISTRY: &str = "us-docker.pkg.dev/openrouter-ci/harbor";

const WORLD_INPUTS: &[&str] = &[
    ".dockerignore",
    "Cargo.toml",
    "Cargo.lock",
    "crates",
    "scenarios",
    "images/world",
];
const TOOLS_INPUTS: &[&str] = &[
    ".dockerignore",
    "Cargo.toml",
    "Cargo.lock",
    "crates",
    "catalog/services.toml",
    "servers",
    "images/tools",
];
/// Directory names the build context excludes (see `.dockerignore`).
const SKIPPED_DIRS: &[&str] = &["target", ".git", "node_modules"];
const TAG_HEX_LEN: usize = 16;

/// The published images for the sources under `root` (the tool-bench directory).
pub fn published(root: &Path) -> anyhow::Result<Images> {
    Ok(Images {
        world: world(root)?,
        tools: tools(root)?,
    })
}

/// The published world image for the sources under `root`.
pub fn world(root: &Path) -> anyhow::Result<String> {
    Ok(format!(
        "{REGISTRY}/tool-bench-world:{}",
        content_tag(root, WORLD_INPUTS)?
    ))
}

/// The published tools image for the sources under `root`.
pub fn tools(root: &Path) -> anyhow::Result<String> {
    Ok(format!(
        "{REGISTRY}/tool-bench-tools:{}",
        content_tag(root, TOOLS_INPUTS)?
    ))
}

/// Hex prefix of a SHA-256 over every input file's relative path, executable bit and bytes.
pub fn content_tag(root: &Path, inputs: &[&str]) -> anyhow::Result<String> {
    let mut files = Vec::new();
    for input in inputs {
        collect(root, &root.join(input), &mut files)?;
    }
    files.sort();
    let mut hasher = Sha256::new();
    for relative in &files {
        let path = root.join(relative);
        let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0, u8::from(is_executable(&path)?)]);
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(&bytes);
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(hex[..TAG_HEX_LEN].to_owned())
}

fn collect(root: &Path, path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let meta = fs::metadata(path).with_context(|| format!("image input {}", path.display()))?;
    if meta.is_file() {
        files.push(path.strip_prefix(root)?.to_path_buf());
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let skipped = entry.file_type()?.is_dir()
            && SKIPPED_DIRS.contains(&entry.file_name().to_string_lossy().as_ref());
        if !skipped {
            collect(root, &entry.path(), files)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> anyhow::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(path)?.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> anyhow::Result<bool> {
    Ok(false)
}
