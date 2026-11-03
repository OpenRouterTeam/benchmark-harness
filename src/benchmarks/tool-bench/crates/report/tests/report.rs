use std::{fs, path::Path};

use report::{Outcome, Trial, compare, load_trials};
use serde_json::json;

fn write_trial(dir: &Path, name: &str, task: &str, reward: f64, outcome: f64) {
    let d = dir.join(name);
    fs::create_dir_all(&d).unwrap();
    let result = json!({
        "task_name": format!("openrouter/tool-bench__{task}"),
        "trial_name": name,
        "agent_info": {"name": "claude-code", "model_info": {"name": "anthropic/claude-sonnet-5"}},
        "verifier_result": {"rewards": {"reward": reward, "outcome_code": outcome, "api_calls": 3.0, "mcp_bypass": 0.0}},
        "exception_info": null
    });
    fs::write(d.join("result.json"), result.to_string()).unwrap();
}

#[test]
fn loads_trials_recursively_and_ignores_job_level_results() {
    let dir = tempfile::tempdir().unwrap();
    write_trial(dir.path(), "job/a__1", "code__github__x", 1.0, 0.0);
    fs::write(
        dir.path().join("job/result.json"),
        json!({"id": "job", "stats": {}}).to_string(),
    )
    .unwrap();
    let trials = load_trials(dir.path()).unwrap();
    assert_eq!(trials.len(), 1);
    assert_eq!(trials[0].task, "code__github__x");
    assert_eq!(trials[0].outcome, Outcome::Pass);
}

#[test]
fn pairs_tasks_and_reports_differences_with_a_deterministic_interval() {
    let t = |task: &str, reward: f64, outcome: Outcome| Trial {
        task: task.into(),
        reward,
        outcome,
    };
    let a = vec![
        t("code__github__x", 0.0, Outcome::ContextOverflow),
        t("code__github__x", 0.0, Outcome::ContextOverflow),
        t("ops__pagerduty__y", 1.0, Outcome::Pass),
        t("ops__pagerduty__y", 0.0, Outcome::Fail),
        t("only__in__a", 1.0, Outcome::Pass),
    ];
    let b = vec![
        t("code__github__x", 1.0, Outcome::Pass),
        t("code__github__x", 1.0, Outcome::Pass),
        t("ops__pagerduty__y", 1.0, Outcome::Pass),
        t("ops__pagerduty__y", 1.0, Outcome::Pass),
    ];
    let r = compare(&a, &b);
    assert_eq!(r.paired_tasks, 2);
    assert_eq!(r.unpaired_tasks, vec!["only__in__a".to_owned()]);
    assert!((r.mean_a - 0.25).abs() < 1e-9, "{}", r.mean_a);
    assert!((r.mean_b - 1.0).abs() < 1e-9);
    assert!((r.mean_diff - 0.75).abs() < 1e-9);
    assert!(r.ci95.0 <= r.mean_diff && r.mean_diff <= r.ci95.1);
    assert_eq!(r.b_better, 2);
    assert_eq!(r.a_better, 0);
    assert_eq!(r.outcomes_a["context_overflow"], 2);
    assert_eq!(
        compare(&a, &b),
        r,
        "bootstrap is seeded, so reports are reproducible"
    );
    let groups: Vec<&str> = r.groups.iter().map(|g| g.group.as_str()).collect();
    assert_eq!(groups, ["code", "ops"]);
    let md = r.markdown();
    assert!(
        md.contains("| code__github__x | 0.00 | 1.00 | +1.00 |"),
        "{md}"
    );
}
