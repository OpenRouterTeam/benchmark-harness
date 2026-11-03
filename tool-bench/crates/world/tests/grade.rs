use std::{
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

use mock_core::{
    dataset::Dataset,
    world::{WorldState, shared},
};
use serde_json::{Value, json};
use world::{PortChoice, spawn};

#[tokio::test]
async fn grades_a_trial_end_to_end_over_http() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/example/github/basic");
    let state = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    let running = spawn(
        state,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        PortChoice::Ephemeral,
    )
    .await
    .unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let gh = running.services["github"];

    http.post(format!("http://{gh}/api/v3/repos/octo-org/app/issues"))
        .body(r#"{"title":"Login page returns 500"}"#)
        .send()
        .await
        .unwrap();
    http.post(format!("http://{}/_bench/answer", running.admin))
        .body("Opened issue #4: Login page returns 500")
        .send()
        .await
        .unwrap();

    let expect = json!({
        "state": [{"pointer": "/services/github/issues/octo-org~1app#4/title", "equals": "Login page returns 500"}],
        "required_calls": [{"service": "github", "method": "POST", "path": "/api/v3/repos/octo-org/app/issues"}],
        "answer": {"contains": ["#4"]}
    });
    let grade = |tools_host: &'static str| {
        let http = http.clone();
        let expect = expect.clone();
        let admin = running.admin;
        async move {
            http.post(format!(
                "http://{admin}/_bench/grade?tools_host={tools_host}"
            ))
            .json(&expect)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
        }
    };
    let pass = grade("127.0.0.1").await;
    assert_eq!(pass["reward"], 1.0);
    assert_eq!(pass["api_calls"], 1.0);
    assert_eq!(pass["mcp_bypass"], 0.0);

    // From any other "tools" address the same call is a bypass and does not count.
    let bypass = grade("192.0.2.1").await;
    assert_eq!(bypass["reward"], 0.0);
    assert_eq!(bypass["mcp_bypass"], 1.0);
}
