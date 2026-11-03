//! Scenario sources (`scenarios/<group>/<service|multi>/<slug>/`) and their conversion into
//! Harbor task directories.

pub mod taskgen;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use mock_core::{dataset::Dataset, grading::Expect};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub id: String,
    pub title: String,
    pub now: String,
    pub seed: String,
    /// Services whose datasets this scenario provides (all 32 endpoints are always exposed).
    pub services: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_sec: u32,
}

fn default_timeout() -> u32 {
    900
}

/// One oracle tool call, replayed by `solution/solve.sh` through the real MCP endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct SolutionCall {
    pub endpoint: String,
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Solution {
    #[serde(default, rename = "call")]
    pub calls: Vec<SolutionCall>,
    /// Written to `/app/answer.md` by the oracle.
    #[serde(default)]
    pub answer: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct File {
    scenario: Meta,
    #[serde(default)]
    expect: Expect,
    #[serde(default)]
    solution: Solution,
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub dir: PathBuf,
    pub meta: Meta,
    pub instruction: String,
    pub expect: Expect,
    pub solution: Solution,
}

pub const ANSWER_PATH: &str = "/app/answer.md";

impl Scenario {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("scenario.toml");
        let file: File = toml::from_str(
            &fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
        )
        .with_context(|| format!("parsing {}", path.display()))?;
        let instruction = fs::read_to_string(dir.join("instruction.md"))
            .with_context(|| format!("reading {}/instruction.md", dir.display()))?;
        Ok(Self {
            dir: dir.to_owned(),
            meta: file.scenario,
            instruction,
            expect: file.expect,
            solution: file.solution,
        })
    }

    /// Harbor task directory name: the id with `/` replaced by `__` (globbable by group/service).
    pub fn task_name(&self) -> String {
        self.meta.id.replace('/', "__")
    }

    /// Consistency checks against the directory layout, the dataset, and the catalog.
    pub fn validate(&self, root: &Path, catalog: &catalog::Catalog) -> anyhow::Result<()> {
        let expected_dir = root.join(&self.meta.id);
        if fs::canonicalize(&expected_dir).ok() != fs::canonicalize(&self.dir).ok() {
            bail!(
                "{}: id must match its directory under scenarios/",
                self.meta.id
            );
        }
        if self.meta.id.split('/').count() != 3 {
            bail!(
                "{}: id must be <group>/<service|multi>/<slug>",
                self.meta.id
            );
        }
        Dataset::load(&self.dir).with_context(|| format!("{}: dataset", self.meta.id))?;
        for service in &self.meta.services {
            if !catalog.services.iter().any(|s| &s.name == service) {
                bail!("{}: unknown service {service}", self.meta.id);
            }
        }
        for call in &self.solution.calls {
            if catalog.endpoint(&call.endpoint).is_none() {
                bail!(
                    "{}: solution uses unknown endpoint {}",
                    self.meta.id,
                    call.endpoint
                );
            }
        }
        if self.expect.answer.is_some() && !self.instruction.contains(ANSWER_PATH) {
            bail!(
                "{}: expects an answer, so the instruction must ask for {ANSWER_PATH}",
                self.meta.id
            );
        }
        if self.expect.state.is_empty()
            && self.expect.required_calls.is_empty()
            && self.expect.answer.is_none()
        {
            bail!("{}: no expectations", self.meta.id);
        }
        Ok(())
    }
}

/// Every scenario under `root` (directories containing `scenario.toml`), sorted by id.
pub fn discover(root: &Path) -> anyhow::Result<Vec<Scenario>> {
    let mut dirs = Vec::new();
    walk(root, &mut dirs)?;
    let mut scenarios = dirs
        .iter()
        .map(|d| Scenario::load(d))
        .collect::<anyhow::Result<Vec<_>>>()?;
    scenarios.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    Ok(scenarios)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if dir.join("scenario.toml").is_file() {
        out.push(dir.to_owned());
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() && path.file_name().is_some_and(|n| n != "dataset") {
            walk(&path, out)?;
        }
    }
    Ok(())
}
