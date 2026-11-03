//! Paired comparison of two benchmark runs (e.g. router arm A vs arm B) from Harbor trial
//! `result.json` files. Tasks are the pairing unit: each task's mean reward over its
//! attempts in A is compared with the same task in B.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::Serialize;
use serde_json::Value;

/// Grader outcome codes (`mock_core::grading::Outcome::code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
    ContextOverflow,
    /// No reward (trial crashed, timed out, or the grader was unreachable).
    Error,
}

impl Outcome {
    fn from_code(code: Option<f64>) -> Self {
        // Codes are small integers carried as JSON numbers.
        match code.map(|c| c.round() as i64) {
            Some(0) => Self::Pass,
            Some(1) => Self::Fail,
            Some(2) => Self::ContextOverflow,
            _ => Self::Error,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::ContextOverflow => "context_overflow",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trial {
    /// Task directory name, e.g. `code__github__file-login-bug`.
    pub task: String,
    pub reward: f64,
    pub outcome: Outcome,
}

/// Every trial `result.json` under `root` (job-level `result.json` files are skipped).
pub fn load_trials(root: &Path) -> anyhow::Result<Vec<Trial>> {
    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();
    let mut trials = Vec::new();
    for file in files {
        let v: Value = serde_json::from_str(&fs::read_to_string(&file)?)
            .with_context(|| format!("parsing {}", file.display()))?;
        let Some(task_name) = v["task_name"].as_str() else {
            continue;
        };
        let task = task_name
            .rsplit('/')
            .next()
            .unwrap_or(task_name)
            .trim_start_matches("tool-bench__")
            .to_owned();
        let rewards = &v["verifier_result"]["rewards"];
        let outcome = Outcome::from_code(rewards["outcome_code"].as_f64());
        trials.push(Trial {
            task,
            reward: rewards["reward"].as_f64().unwrap_or(0.0),
            outcome,
        });
    }
    Ok(trials)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.file_name().is_some_and(|n| n == "result.json") {
            out.push(path);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskRow {
    pub task: String,
    pub mean_a: f64,
    pub mean_b: f64,
    pub attempts_a: usize,
    pub attempts_b: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupRow {
    pub group: String,
    pub tasks: usize,
    pub mean_a: f64,
    pub mean_b: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    pub paired_tasks: usize,
    pub unpaired_tasks: Vec<String>,
    pub mean_a: f64,
    pub mean_b: f64,
    /// mean(B) − mean(A) over paired tasks.
    pub mean_diff: f64,
    /// 95% paired bootstrap interval of `mean_diff` (10,000 resamples, fixed seed).
    pub ci95: (f64, f64),
    pub b_better: usize,
    pub a_better: usize,
    pub ties: usize,
    pub outcomes_a: BTreeMap<&'static str, usize>,
    pub outcomes_b: BTreeMap<&'static str, usize>,
    pub groups: Vec<GroupRow>,
    pub tasks: Vec<TaskRow>,
}

pub fn compare(a: &[Trial], b: &[Trial]) -> Comparison {
    let by_task = |trials: &[Trial]| {
        let mut m: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for t in trials {
            m.entry(t.task.clone()).or_default().push(t.reward);
        }
        m
    };
    let (ma, mb) = (by_task(a), by_task(b));
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let tasks: Vec<TaskRow> = ma
        .iter()
        .filter_map(|(task, ra)| {
            mb.get(task).map(|rb| TaskRow {
                task: task.clone(),
                mean_a: mean(ra),
                mean_b: mean(rb),
                attempts_a: ra.len(),
                attempts_b: rb.len(),
            })
        })
        .collect();
    let unpaired_tasks = ma
        .keys()
        .chain(mb.keys())
        .filter(|t| !(ma.contains_key(*t) && mb.contains_key(*t)))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let diffs: Vec<f64> = tasks.iter().map(|t| t.mean_b - t.mean_a).collect();
    let n = tasks.len().max(1) as f64;
    let mean_a = tasks.iter().map(|t| t.mean_a).sum::<f64>() / n;
    let mean_b = tasks.iter().map(|t| t.mean_b).sum::<f64>() / n;
    let mut groups: BTreeMap<String, Vec<&TaskRow>> = BTreeMap::new();
    for t in &tasks {
        groups
            .entry(t.task.split("__").next().unwrap_or("").to_owned())
            .or_default()
            .push(t);
    }
    let outcomes = |trials: &[Trial]| {
        let mut m = BTreeMap::new();
        for t in trials {
            *m.entry(t.outcome.key()).or_insert(0) += 1;
        }
        m
    };
    Comparison {
        paired_tasks: tasks.len(),
        unpaired_tasks,
        mean_a,
        mean_b,
        mean_diff: mean_b - mean_a,
        ci95: bootstrap_ci(&diffs),
        b_better: diffs.iter().filter(|d| **d > 0.0).count(),
        a_better: diffs.iter().filter(|d| **d < 0.0).count(),
        ties: diffs.iter().filter(|d| **d == 0.0).count(),
        outcomes_a: outcomes(a),
        outcomes_b: outcomes(b),
        groups: groups
            .into_iter()
            .map(|(group, rows)| GroupRow {
                tasks: rows.len(),
                mean_a: rows.iter().map(|r| r.mean_a).sum::<f64>() / rows.len() as f64,
                mean_b: rows.iter().map(|r| r.mean_b).sum::<f64>() / rows.len() as f64,
                group,
            })
            .collect(),
        tasks,
    }
}

/// Percentile bootstrap of the mean, with a fixed-seed xorshift so reports are reproducible.
fn bootstrap_ci(diffs: &[f64]) -> (f64, f64) {
    if diffs.is_empty() {
        return (0.0, 0.0);
    }
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut means: Vec<f64> = (0..10_000)
        .map(|_| {
            (0..diffs.len())
                .map(|_| diffs[(next() % diffs.len() as u64) as usize])
                .sum::<f64>()
                / diffs.len() as f64
        })
        .collect();
    means.sort_by(f64::total_cmp);
    (means[249], means[9_749])
}

impl Comparison {
    pub fn markdown(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "## Paired comparison (B − A)\n\n- Paired tasks: {}\n- Mean reward: A {:.3}, B {:.3}\n- Difference: {:+.3} (95% CI {:+.3} to {:+.3})\n- Tasks where B is better / A is better / tie: {} / {} / {}\n",
            self.paired_tasks, self.mean_a, self.mean_b, self.mean_diff, self.ci95.0, self.ci95.1, self.b_better, self.a_better, self.ties
        ));
        if !self.unpaired_tasks.is_empty() {
            s.push_str(&format!(
                "- Unpaired (in one run only): {}\n",
                self.unpaired_tasks.join(", ")
            ));
        }
        s.push_str("\n### Outcomes (trials)\n\n| Outcome | A | B |\n|---|---|---|\n");
        for key in ["pass", "fail", "context_overflow", "error"] {
            s.push_str(&format!(
                "| {key} | {} | {} |\n",
                self.outcomes_a.get(key).copied().unwrap_or(0),
                self.outcomes_b.get(key).copied().unwrap_or(0)
            ));
        }
        s.push_str("\n### By group\n\n| Group | Tasks | A | B | Δ |\n|---|---|---|---|---|\n");
        for g in &self.groups {
            s.push_str(&format!(
                "| {} | {} | {:.2} | {:.2} | {:+.2} |\n",
                g.group,
                g.tasks,
                g.mean_a,
                g.mean_b,
                g.mean_b - g.mean_a
            ));
        }
        s.push_str("\n### By task\n\n| Task | A | B | Δ |\n|---|---|---|---|\n");
        for t in &self.tasks {
            s.push_str(&format!(
                "| {} | {:.2} | {:.2} | {:+.2} |\n",
                t.task,
                t.mean_a,
                t.mean_b,
                t.mean_b - t.mean_a
            ));
        }
        s
    }
}
