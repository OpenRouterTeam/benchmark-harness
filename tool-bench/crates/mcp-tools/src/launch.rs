use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// How to start one stdio MCP server. The child gets exactly `env` plus the parent's
/// `PATH` and `HOME`; nothing else leaks in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Parent variables passed through to every child.
pub const INHERITED_ENV: [&str; 2] = ["PATH", "HOME"];
