//! bench-report <run-a-dir> <run-b-dir> [--json]
//!
//! Each dir holds Harbor trial results (local `harbor run -o` output, or a run's artifacts
//! downloaded from the run's artifact bucket).

use std::path::PathBuf;

use anyhow::Context;
use report::{compare, load_trials};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a = PathBuf::from(
        args.first()
            .context("usage: bench-report <run-a> <run-b> [--json]")?,
    );
    let b = PathBuf::from(
        args.get(1)
            .context("usage: bench-report <run-a> <run-b> [--json]")?,
    );
    let report = compare(&load_trials(&a)?, &load_trials(&b)?);
    if args.iter().any(|x| x == "--json") {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.markdown());
    }
    Ok(())
}
