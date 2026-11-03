use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
};

use mock_core::{
    dataset::Dataset,
    world::{WorldState, shared},
};
use serde_json::{Value, json};
use world::{PortChoice, Running, spawn};

async fn start() -> Running {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/example/github/basic");
    let state = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    spawn(
        state,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        PortChoice::Ephemeral,
    )
    .await
    .unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let resp = client()
        .get(format!("http://{addr}{path}"))
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

async fn post(addr: SocketAddr, path: &str, body: &str) -> (u16, String) {
    let resp = client()
        .post(format!("http://{addr}{path}"))
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

/// The request sequence a scenario might drive: read, create, read again.
async fn drive(running: &Running) -> Vec<(u16, String)> {
    let gh = running.services["github"];
    vec![
        get(gh, "/api/v3/repos/octo-org/app/issues").await,
        post(
            gh,
            "/api/v3/repos/octo-org/app/issues",
            r#"{"title":"Login page returns 500"}"#,
        )
        .await,
        get(gh, "/api/v3/repos/octo-org/app/issues?state=all").await,
    ]
}

#[tokio::test]
async fn serves_github_and_admin_over_tcp() {
    let running = start().await;
    let responses = drive(&running).await;
    assert_eq!(
        responses.iter().map(|r| r.0).collect::<Vec<_>>(),
        [200, 201, 200]
    );

    let (_, journal) = get(running.admin, "/_bench/journal").await;
    let journal: Value = serde_json::from_str(&journal).unwrap();
    let entries = journal["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[1]["method"], "POST");
    assert_eq!(entries[1]["source"], "127.0.0.1");
    assert_eq!(entries[1]["service"], "github");

    let (_, state) = get(running.admin, "/_bench/state").await;
    let state: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(
        state["services"]["github"]["issues"]["octo-org/app#4"]["title"],
        "Login page returns 500"
    );

    let (status, body) = post(running.admin, "/_bench/reset", "").await;
    assert_eq!(
        (status, serde_json::from_str::<Value>(&body).unwrap()),
        (200, json!({"status": "reset"}))
    );
    let (_, after) = get(
        running.services["github"],
        "/api/v3/repos/octo-org/app/issues?state=all",
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&after)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn admin_is_not_served_on_service_ports() {
    let running = start().await;
    let (status, _) = get(running.services["github"], "/_bench/state").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn two_fresh_worlds_answer_identically() {
    let a = start().await;
    let b = start().await;
    let ra = drive(&a).await;
    let rb = drive(&b).await;
    // Bodies contain the Host header (port differs per world), so compare with it normalised.
    let normalise = |rs: Vec<(u16, String)>, port: u16| -> Vec<(u16, String)> {
        rs.into_iter()
            .map(|(s, b)| (s, b.replace(&format!("127.0.0.1:{port}"), "HOST")))
            .collect()
    };
    assert_eq!(
        normalise(ra, a.services["github"].port()),
        normalise(rb, b.services["github"].port())
    );
}

#[tokio::test]
async fn admin_token_guards_everything_but_health_and_ca() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/example/github/basic");
    let state = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    let running = world::spawn_guarded(
        state,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        PortChoice::Ephemeral,
        Some("s3cret".to_owned()),
    )
    .await
    .unwrap();
    let admin = running.admin;
    assert_eq!(get(admin, "/_bench/health").await.0, 200);
    assert_eq!(get(admin, "/_bench/ca.pem").await.0, 200);
    for path in ["/_bench/state", "/_bench/journal"] {
        assert_eq!(get(admin, path).await.0, 401, "{path}");
    }
    assert_eq!(post(admin, "/_bench/reset", "").await.0, 401);
    assert_eq!(post(admin, "/_bench/answer", "x").await.0, 401);
    let wrong = client()
        .get(format!("http://{admin}/_bench/state"))
        .bearer_auth("nope")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status().as_u16(), 401);
    let right = client()
        .get(format!("http://{admin}/_bench/state"))
        .bearer_auth("s3cret")
        .send()
        .await
        .unwrap();
    assert_eq!(right.status().as_u16(), 200);
}
