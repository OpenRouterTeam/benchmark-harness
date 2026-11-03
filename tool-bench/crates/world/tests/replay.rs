use std::{
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

use mock_core::{
    dataset::Dataset,
    journal::JournalEntry,
    world::{WorldState, shared},
};
use world::{PortChoice, replay::replay, spawn};

#[tokio::test]
async fn a_recorded_session_replays_identically_including_tls_and_form_bodies() {
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/code/github/file-login-bug");
    let state = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    let running = spawn(
        state.clone(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        PortChoice::Ephemeral,
    )
    .await
    .unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let gh = running.services["github"];
    let okta = running.services["okta"];
    http.get(format!(
        "http://{gh}/api/v3/repos/octo-org/app/issues?state=all&per_page=2"
    ))
    .header("x-forwarded-proto", "https")
    .header("host", "github.octo-corp.com")
    .send()
    .await
    .unwrap();
    http.post(format!("http://{gh}/api/v3/repos/octo-org/app/issues"))
        .header("content-type", "application/json")
        .body(r#"{"title":"Replay me","body":"x"}"#)
        .send()
        .await
        .unwrap();
    http.post(format!("http://{okta}/oauth2/v1/token"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body("grant_type=refresh_token")
        .send()
        .await
        .unwrap();
    http.get(format!("http://{gh}/nope")).send().await.unwrap();

    let entries: Vec<JournalEntry> = state.lock().unwrap().journal.entries().to_vec();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].scheme, "https");
    assert_eq!(entries[1].content_type.as_deref(), Some("application/json"));
    assert_eq!(replay(&dir, &entries).await.unwrap(), vec![]);

    // A tampered recording is caught.
    let mut tampered = entries.clone();
    tampered[1].response_body["number"] = serde_json::json!(99);
    let mismatches = replay(&dir, &tampered).await.unwrap();
    assert_eq!(mismatches.len(), 1);
    assert_eq!(mismatches[0].seq, 2);
}
