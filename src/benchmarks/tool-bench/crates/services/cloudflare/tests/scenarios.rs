//! In-process oracle for `scenarios/cloud/cloudflare/*`: replays each scenario's
//! `[[solution.call]]` against the mock and grades the result with the real grader. The
//! solution must pass and doing nothing must fail (what `deploy/oracle.sh` checks end to end).

use std::path::{Path, PathBuf};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use mock_cloudflare::Cloudflare;
use mock_core::{
    dataset::Dataset,
    grading::{Expect, GradeInput, grade},
    serve::service_app,
    world::{WorldState, shared},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    expect: Expect,
    #[serde(default)]
    solution: Solution,
}

#[derive(Default, Deserialize)]
struct Solution {
    #[serde(default, rename = "call")]
    calls: Vec<Call>,
    #[serde(default)]
    answer: Option<String>,
}

#[derive(Deserialize)]
struct Call {
    endpoint: String,
    tool: String,
    #[serde(default)]
    arguments: Value,
}

fn scenario_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scenarios/cloud/cloudflare");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("scenario.toml").exists())
        .collect();
    dirs.sort();
    dirs
}

async fn call(app: &Router, tool: &str, args: &Value) -> Value {
    let req = Request::post("/_mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "name": tool, "arguments": args }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn run(dir: &Path, solve: bool) -> (f64, Vec<String>) {
    let file: File =
        toml::from_str(&std::fs::read_to_string(dir.join("scenario.toml")).unwrap()).unwrap();
    let world = shared(WorldState::from_dataset(Dataset::load(dir).unwrap()).unwrap());
    let app = service_app(world.clone(), &Cloudflare);
    let mut errors = Vec::new();
    if solve {
        for c in &file.solution.calls {
            assert_eq!(c.endpoint, "cloudflare");
            let result = call(&app, &c.tool, &c.arguments).await;
            if result["isError"] == true {
                errors.push(
                    result["content"][0]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                );
            }
        }
    }
    let w = world.lock().unwrap();
    let answer = if solve {
        file.solution.answer.as_deref()
    } else {
        None
    };
    let g = grade(&GradeInput {
        expect: &file.expect,
        state: w.snapshot(),
        journal: w.journal.entries(),
        answer,
        tools_ips: vec!["unknown".to_owned()],
        context_overflow: false,
    });
    (g.reward, errors)
}

#[tokio::test]
async fn every_scenario_passes_with_its_solution_and_fails_without() {
    let dirs = scenario_dirs();
    assert_eq!(dirs.len(), 4, "{dirs:?}");
    for dir in dirs {
        let (reward, errors) = run(&dir, true).await;
        assert!(
            errors.is_empty(),
            "{}: tool errors {errors:?}",
            dir.display()
        );
        assert_eq!(reward, 1.0, "{}: oracle", dir.display());
        let (reward, _) = run(&dir, false).await;
        assert_eq!(reward, 0.0, "{}: nop", dir.display());
    }
}
