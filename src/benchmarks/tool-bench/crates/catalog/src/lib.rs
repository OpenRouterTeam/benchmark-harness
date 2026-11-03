//! Parser and validator for `catalog/services.toml`, the source of truth for every service:
//! its mock API port, TLS hostnames, and the MCP endpoints the agent connects to.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, bail};
use mcp_tools::launch::LaunchSpec;
use serde::Deserialize;

/// Hostname of the world container on the trial's compose network.
pub const WORLD_HOST: &str = "world";
/// Directory holding per-service config files inside the tools container.
pub const CONFIG_ROOT: &str = "/etc/tool-bench";
/// Env every server gets: fixed Python hash seed, so set/dict-derived text is stable.
pub const DETERMINISM_ENV: [(&str, &str); 1] = [("PYTHONHASHSEED", "0")];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Real,
    RealTls,
    Replica,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Runtime {
    Npm,
    Pypi,
    Go,
    Cargo,
    GitNpm,
    GitPypi,
    Replica,
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pypi => "pypi",
            Self::Go => "go",
            Self::Cargo => "cargo",
            Self::GitNpm => "git-npm",
            Self::GitPypi => "git-pypi",
            Self::Replica => "replica",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Endpoint {
    pub id: String,
    pub mcp_port: u16,
    pub runtime: Runtime,
    pub package: Option<String>,
    pub version: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub snapshot: Option<String>,
    pub source: Option<String>,
    /// The server lists tools in a nondeterministic order (e.g. from a hash map); the bridge
    /// returns its `tools/list` sorted by name. Definitions are never changed.
    #[serde(default)]
    pub sort_tools: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Service {
    pub name: String,
    pub group: String,
    pub mode: Mode,
    pub world_port: u16,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(rename = "endpoint", default)]
    pub endpoints: Vec<Endpoint>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    #[serde(rename = "service")]
    pub services: Vec<Service>,
}

impl Catalog {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// Every problem found, joined into one error.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut problems = Vec::new();
        let mut seen = |kind: &str, value: String, set: &mut BTreeSet<String>| {
            if !set.insert(value.clone()) {
                problems.push(format!("{kind} {value} used twice"));
            }
        };
        let (mut names, mut world_ports, mut ids, mut mcp_ports, mut hosts) = Default::default();
        for s in &self.services {
            seen("service name", s.name.clone(), &mut names);
            seen("world_port", s.world_port.to_string(), &mut world_ports);
            for h in &s.hosts {
                seen("host", h.clone(), &mut hosts);
            }
            for e in &s.endpoints {
                seen("endpoint id", e.id.clone(), &mut ids);
                seen("mcp_port", e.mcp_port.to_string(), &mut mcp_ports);
            }
        }
        for s in &self.services {
            if s.endpoints.is_empty() {
                problems.push(format!("{}: no endpoints", s.name));
            }
            for e in &s.endpoints {
                for text in e.args.iter().chain(e.env.values()) {
                    if let Err(e) = substitute(text, s) {
                        problems.push(format!("{}: {e}", s.name));
                    }
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            bail!("{}", problems.join("; "))
        }
    }

    pub fn endpoint(&self, id: &str) -> Option<(&Service, &Endpoint)> {
        self.services
            .iter()
            .flat_map(|s| s.endpoints.iter().map(move |e| (s, e)))
            .find(|(_, e)| e.id == id)
    }

    /// Stdio launch spec for a non-replica endpoint, placeholders substituted.
    pub fn launch(&self, id: &str) -> anyhow::Result<LaunchSpec> {
        let (service, endpoint) = self
            .endpoint(id)
            .with_context(|| format!("no endpoint {id}"))?;
        let command = endpoint
            .command
            .clone()
            .with_context(|| format!("{id} is a replica and has no launch spec"))?;
        Ok(LaunchSpec {
            command,
            args: endpoint
                .args
                .iter()
                .map(|a| substitute(a, service))
                .collect::<anyhow::Result<_>>()?,
            env: DETERMINISM_ENV
                .iter()
                .map(|(k, v)| Ok(((*k).to_owned(), (*v).to_owned())))
                .chain(
                    endpoint
                        .env
                        .iter()
                        .map(|(k, v)| Ok((k.clone(), substitute(v, service)?))),
                )
                .collect::<anyhow::Result<_>>()?,
        })
    }

    /// Network aliases the world container needs: every TLS hostname, sorted.
    pub fn world_aliases(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self.services.iter().flat_map(|s| s.hosts.clone()).collect();
        hosts.sort();
        hosts
    }
}

/// Replaces `{world}`, `{world_host}` and `{config}`; any other `{...}` is an error.
fn substitute(text: &str, service: &Service) -> anyhow::Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .map(|e| start + e)
            .context("unclosed {")?;
        let value = match &rest[start + 1..end] {
            "world" => format!("http://{WORLD_HOST}:{}", service.world_port),
            "world_host" => format!("{WORLD_HOST}:{}", service.world_port),
            "config" => format!("{CONFIG_ROOT}/{}", service.name),
            other => bail!("unknown placeholder {{{other}}}"),
        };
        out.push_str(&value);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
