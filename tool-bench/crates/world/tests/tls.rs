use std::{
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

use mock_core::{
    dataset::Dataset,
    world::{WorldState, shared},
};
use serde_json::Value;
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

/// A client that trusts only the world's CA and resolves `host` to the world's TLS port.
fn client(running: &Running, host: &str) -> reqwest::Client {
    let ca = reqwest::Certificate::from_pem(running.ca_pem.as_bytes()).unwrap();
    reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([ca])
        .resolve(host, running.tls)
        .build()
        .unwrap()
}

#[tokio::test]
async fn serves_real_hostnames_over_tls_with_the_local_ca() {
    let running = start().await;
    let resp = client(&running, "api.github.com")
        .get(format!(
            "https://api.github.com:{}/api/v3/user",
            running.tls.port()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["login"], "mona");

    let journal: Value = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{}/_bench/journal", running.admin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(journal["entries"][0]["service"], "github");
    assert_eq!(journal["entries"][0]["path"], "/api/v3/user");
}

#[tokio::test]
async fn absolute_urls_use_https_behind_tls() {
    let running = start().await;
    let issues: Value = client(&running, "api.github.com")
        .get(format!(
            "https://api.github.com:{}/api/v3/repos/octo-org/app/issues?per_page=1",
            running.tls.port()
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Stored fixture URLs are untouched; only generated links use the request scheme.
    assert_eq!(issues.as_array().unwrap().len(), 1);
    let resp = client(&running, "api.github.com")
        .get(format!(
            "https://api.github.com:{}/api/v3/repos/octo-org/app/issues?per_page=1",
            running.tls.port()
        ))
        .send()
        .await
        .unwrap();
    let link = resp.headers()["link"].to_str().unwrap().to_owned();
    assert!(link.starts_with("<https://api.github.com:"), "{link}");
}

#[tokio::test]
async fn hosts_outside_the_certificate_fail_tls() {
    let running = start().await;
    let err = client(&running, "evil.example")
        .get(format!("https://evil.example:{}/", running.tls.port()))
        .send()
        .await
        .unwrap_err();
    assert!(err.is_connect() || err.is_request(), "{err:?}");
}

#[tokio::test]
async fn ca_certificate_is_published_on_admin() {
    let running = start().await;
    let pem = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{}/_bench/ca.pem", running.admin))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(pem, running.ca_pem);
    assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
}
