//! world-replay <scenario-dir> <journal.json>
//!
//! `journal.json` is the output of `GET /_bench/journal` from a trial. Exits non-zero if any
//! replayed response differs from the recording.

use std::path::PathBuf;

use anyhow::Context;
use mock_core::journal::JournalEntry;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: world-replay <scenario-dir> <journal.json>";
    let dir = PathBuf::from(args.first().context(usage)?);
    let file = PathBuf::from(args.get(1).context(usage)?);
    let journal: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
    let entries: Vec<JournalEntry> = serde_json::from_value(journal["entries"].clone())?;
    let mismatches = world::replay::replay(&dir, &entries).await?;
    for m in &mismatches {
        println!("seq {}: {}", m.seq, m.detail);
    }
    println!(
        "{} requests replayed, {} mismatches",
        entries.len(),
        mismatches.len()
    );
    if !mismatches.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}
