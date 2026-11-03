use std::{fs, path::Path};

use catalog::Catalog;
use scenario::{
    Scenario, discover,
    taskgen::{Images, write_task},
};

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn catalog() -> Catalog {
    Catalog::load(&root().join("catalog/services.toml")).unwrap()
}

#[test]
fn every_scenario_in_the_repo_validates() {
    let scenarios = discover(&root().join("scenarios")).unwrap();
    assert!(!scenarios.is_empty());
    for s in &scenarios {
        s.validate(&root().join("scenarios"), &catalog()).unwrap();
    }
}

#[test]
fn generated_task_has_harbor_layout_and_all_endpoints() {
    let s = Scenario::load(&root().join("scenarios/code/github/file-login-bug")).unwrap();
    let out = tempfile::tempdir().unwrap();
    let images = Images {
        world: "w:1".into(),
        tools: "t:1".into(),
    };
    write_task(&s, &catalog(), &images, out.path()).unwrap();
    let task = out.path().join("code__github__file-login-bug");
    for f in [
        "task.toml",
        "instruction.md",
        "environment/Dockerfile",
        "environment/docker-compose.yaml",
        "tests/test.sh",
        "tests/expect.json",
        "solution/solve.sh",
        "solution/call-0.json",
    ] {
        assert!(task.join(f).is_file(), "{f}");
    }
    let toml = fs::read_to_string(task.join("task.toml")).unwrap();
    assert_eq!(toml.matches("[[environment.mcp_servers]]").count(), 32);
    assert!(toml.contains("url = \"http://tools:7101/mcp\""));
    let compose = fs::read_to_string(task.join("environment/docker-compose.yaml")).unwrap();
    assert!(compose.contains("image: w:1") && compose.contains("image: t:1"));
    assert!(compose.contains("- github.octo-corp.com"));
    assert!(compose.contains("SCENARIO_ID: \"code/github/file-login-bug\""));
    let call: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(task.join("solution/call-0.json")).unwrap())
            .unwrap();
    assert_eq!(call["params"]["name"], "issue_write");
    let test_sh = fs::read_to_string(task.join("tests/test.sh")).unwrap();
    assert!(test_sh.contains("/_bench/grade") && test_sh.contains("journal.json"));
    assert!(test_sh.contains("Bearer ${HARBOR_TRIAL_ADMIN_TOKEN:-}"));
    assert!(compose.contains("BENCH_ADMIN_TOKEN: \"${HARBOR_TRIAL_ADMIN_TOKEN:-}\""));
    assert!(!task.join("tests/admin-token").exists());
}

#[test]
fn published_image_tags_follow_their_sources() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (file, body) in [
        ("Cargo.toml", "a"),
        ("Cargo.lock", "b"),
        ("crates/x.rs", "c"),
        ("scenarios/s.toml", "d"),
        ("images/world/Dockerfile", "e"),
        ("images/tools/Dockerfile", "f"),
        ("catalog/services.toml", "g"),
        ("servers/n.json", "h"),
    ] {
        fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        fs::write(root.join(file), body).unwrap();
    }
    let before = scenario::images::published(root).unwrap();
    assert!(before.world.starts_with("us-docker.pkg.dev/openrouter-ci/harbor/tool-bench-world:"));
    assert!(before.tools.starts_with("us-docker.pkg.dev/openrouter-ci/harbor/tool-bench-tools:"));

    fs::create_dir_all(root.join("crates/target")).unwrap();
    fs::write(root.join("crates/target/junk"), "ignored").unwrap();
    assert_eq!(scenario::images::published(root).unwrap().world, before.world);

    fs::write(root.join("scenarios/s.toml"), "changed").unwrap();
    let after = scenario::images::published(root).unwrap();
    assert_ne!(after.world, before.world);
    assert_eq!(after.tools, before.tools);
}

#[test]
fn validation_rejects_answers_without_the_answer_path_and_bad_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let id = "code/github/bad";
    let d = dir.path().join(id);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("instruction.md"), "Tell me the count.").unwrap();
    fs::write(
        d.join("scenario.toml"),
        format!("[scenario]\nid = \"{id}\"\ntitle = \"t\"\nnow = \"2026-01-01T00:00:00Z\"\nseed = \"s\"\nservices = [\"github\"]\n\n[expect.answer]\ncontains = [\"3\"]\n"),
    )
    .unwrap();
    let s = Scenario::load(&d).unwrap();
    let err = s.validate(dir.path(), &catalog()).unwrap_err().to_string();
    assert!(err.contains("/app/answer.md"), "{err}");

    fs::write(d.join("instruction.md"), "Write it to /app/answer.md.").unwrap();
    fs::write(
        d.join("scenario.toml"),
        format!("[scenario]\nid = \"{id}\"\ntitle = \"t\"\nnow = \"2026-01-01T00:00:00Z\"\nseed = \"s\"\nservices = [\"github\"]\n\n[expect.answer]\ncontains = [\"3\"]\n\n[[solution.call]]\nendpoint = \"nope\"\ntool = \"x\"\n"),
    )
    .unwrap();
    let err = Scenario::load(&d)
        .unwrap()
        .validate(dir.path(), &catalog())
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown endpoint nope"), "{err}");
}
