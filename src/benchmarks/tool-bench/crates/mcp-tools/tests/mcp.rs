use std::{collections::BTreeMap, net::SocketAddr};

use mcp_tools::{
    bridge::StdioBackend,
    client::McpClient,
    front,
    http_client::HttpClient,
    launch::LaunchSpec,
    snapshot::{capture, o200k_count, sha256_hex},
    stdio::StdioConn,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

fn fake(env: &[(&str, &str)]) -> LaunchSpec {
    LaunchSpec {
        command: env!("CARGO_BIN_EXE_fake-mcp-server").to_owned(),
        args: vec![],
        env: env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    }
}

async fn stdio_client(env: &[(&str, &str)]) -> McpClient {
    McpClient::Stdio(StdioConn::spawn(&fake(env)).await.unwrap())
}

fn tool_text(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn stdio_round_trip_skips_non_json_output() {
    let mut client = stdio_client(&[]).await;
    let init = client
        .request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
        )
        .await
        .unwrap();
    assert_eq!(init["serverInfo"]["name"], "fake");
    let echo = client
        .request("tools/call", json!({"name": "echo", "arguments": {"a": 1}}))
        .await
        .unwrap();
    assert_eq!(tool_text(&echo), r#"{"a":1}"#);
}

#[tokio::test]
async fn stdio_errors_surface_as_err() {
    let mut client = stdio_client(&[]).await;
    let err = client
        .request("tools/call", json!({"name": "nope"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown tool nope"), "{err}");
}

#[tokio::test]
async fn child_env_is_exactly_configured_plus_path_and_home() {
    let mut client = stdio_client(&[("FAKE_TOKEN", "abc")]).await;
    let env = client
        .request("tools/call", json!({"name": "env", "arguments": {}}))
        .await
        .unwrap();
    let keys = tool_text(&env);
    let keys: Vec<&str> = keys.split(',').collect();
    assert!(keys.contains(&"FAKE_TOKEN"));
    assert!(
        keys.iter()
            .all(|k| ["FAKE_TOKEN", "PATH", "HOME"].contains(k)),
        "{keys:?}"
    );
}

#[tokio::test]
async fn server_initiated_requests_get_method_not_found() {
    let mut client = stdio_client(&[]).await;
    let roots = client
        .request("tools/call", json!({"name": "roots", "arguments": {}}))
        .await
        .unwrap();
    let reply: Value = serde_json::from_str(&tool_text(&roots)).unwrap();
    assert_eq!(reply["id"], "srv-1");
    assert_eq!(reply["error"]["code"], -32601);
}

#[tokio::test]
async fn capture_follows_pagination_and_counts_tokens() {
    let mut client = stdio_client(&[("FAKE_TOOL_PAGES", "3")]).await;
    let snap = capture(&mut client, "fake").await.unwrap();
    let names: Vec<&str> = snap
        .tools
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "tool_0_0", "tool_0_1", "tool_1_0", "tool_1_1", "tool_2_0", "tool_2_1"
        ]
    );
    assert_eq!(snap.counts.tools, 6);
    assert_eq!(snap.server_info["name"], "fake");
    let bytes = serde_json::to_vec(&snap.tools).unwrap();
    assert_eq!(snap.counts.bytes, bytes.len());
    assert_eq!(snap.sha256, sha256_hex(&bytes));

    let mut again = stdio_client(&[("FAKE_TOOL_PAGES", "3")]).await;
    assert_eq!(
        capture(&mut again, "fake").await.unwrap().sha256,
        snap.sha256
    );
}

#[test]
fn o200k_counts_known_text() {
    assert_eq!(o200k_count("hello world"), 2);
}

async fn start_bridge(env: &[(&str, &str)]) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = front::router(StdioBackend::new(fake(env)));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

#[tokio::test]
async fn bridge_serves_stdio_server_over_streamable_http() {
    let addr = start_bridge(&[("FAKE_TOOL_PAGES", "2")]).await;
    let mut via_http = McpClient::Http(HttpClient::new(format!("http://{addr}/mcp")));
    let snap_http = capture(&mut via_http, "fake").await.unwrap();
    let mut via_stdio = stdio_client(&[("FAKE_TOOL_PAGES", "2")]).await;
    let snap_stdio = capture(&mut via_stdio, "fake").await.unwrap();
    assert_eq!(snap_http.tools, snap_stdio.tools);
    assert_eq!(snap_http.sha256, snap_stdio.sha256);
}

#[tokio::test]
async fn bridge_sessions_are_isolated_and_deletable() {
    let addr = start_bridge(&[]).await;
    let url = format!("http://{addr}/mcp");
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}});

    let open = |http: reqwest::Client, url: String, init: Value| async move {
        let resp = http.post(&url).json(&init).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        resp.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned()
    };
    let a = open(http.clone(), url.clone(), init.clone()).await;
    let b = open(http.clone(), url.clone(), init.clone()).await;
    assert_ne!(a, b);

    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let ok = http
        .post(&url)
        .header("mcp-session-id", &a)
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.json::<Value>().await.unwrap()["id"], 2);

    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let accepted = http
        .post(&url)
        .header("mcp-session-id", &a)
        .json(&initialized)
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 202);

    let deleted = http
        .delete(&url)
        .header("mcp-session-id", &a)
        .send()
        .await
        .unwrap();
    assert!(deleted.status().is_success());
    let gone = http
        .post(&url)
        .header("mcp-session-id", &a)
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);
    let still = http
        .post(&url)
        .header("mcp-session-id", &b)
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(still.status(), 200);

    let missing = http.post(&url).json(&list).send().await.unwrap();
    assert_eq!(missing.status(), 400);
}

mod replica {
    use mcp_tools::{
        client::McpClient,
        front,
        http_client::HttpClient,
        replica::ReplicaBackend,
        snapshot::{Snapshot, capture},
    };
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    fn snapshot() -> Snapshot {
        Snapshot::new(
            "acme",
            json!({"name": "acme-mcp", "version": "9.9.9", "source": "test"}),
            Value::Null,
            vec![
                json!({"name": "acme_read", "description": "Read.", "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}}}),
                json!({"name": "acme_write", "description": "Write.", "inputSchema": {"type": "object"}, "annotations": {"destructiveHint": true}}),
            ],
        )
    }

    async fn start() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = front::router(ReplicaBackend::new(snapshot()));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/mcp")
    }

    #[tokio::test]
    async fn serves_the_snapshot_verbatim() {
        let url = start().await;
        let mut client = McpClient::Http(HttpClient::new(url));
        let captured = capture(&mut client, "acme").await.unwrap();
        let original = snapshot();
        assert_eq!(captured.tools, original.tools);
        assert_eq!(captured.sha256, original.sha256);
        assert_eq!(
            captured.server_info,
            json!({"name": "acme-mcp", "version": "9.9.9"})
        );
    }

    #[tokio::test]
    async fn forwards_tool_calls_to_the_world() {
        use axum::{Json, Router, routing::post};
        let world = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let world_addr = world.local_addr().unwrap();
        let app = Router::new().route(
            mcp_tools::replica::TOOLS_CALL_PATH,
            post(|Json(call): Json<Value>| async move {
                Json(json!({"content": [{"type": "text", "text": format!("{} {}", call["name"], call["arguments"]["id"])}]}))
            }),
        );
        tokio::spawn(async move { axum::serve(world, app).await.unwrap() });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let backend = ReplicaBackend::new(snapshot()).forward_to(format!(
            "http://{world_addr}{}",
            mcp_tools::replica::TOOLS_CALL_PATH
        ));
        tokio::spawn(async move { axum::serve(listener, front::router(backend)).await.unwrap() });

        let mut client = McpClient::Http(HttpClient::new(format!("http://{addr}/mcp")));
        capture(&mut client, "acme").await.unwrap();
        let result = client
            .request(
                "tools/call",
                json!({"name": "acme_read", "arguments": {"id": "42"}}),
            )
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "\"acme_read\" \"42\"");
    }

    #[tokio::test]
    async fn calls_without_a_handler_are_tool_errors_and_unknown_tools_are_rpc_errors() {
        let url = start().await;
        let mut client = McpClient::Http(HttpClient::new(url));
        capture(&mut client, "acme").await.unwrap();
        let result = client
            .request(
                "tools/call",
                json!({"name": "acme_read", "arguments": {"id": "1"}}),
            )
            .await
            .unwrap();
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["content"][0]["text"],
            "acme_read is not available in this scenario"
        );
        let err = client
            .request("tools/call", json!({"name": "nope", "arguments": {}}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("-32602"), "{err}");
        let err = client
            .request("resources/list", json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("-32601"), "{err}");
        assert_eq!(client.request("ping", json!({})).await.unwrap(), json!({}));
    }
}

#[test]
fn sort_tools_by_name_orders_only_the_tool_list() {
    let mut response = json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": [
        {"name": "b", "description": "B", "inputSchema": {"type": "object"}},
        {"name": "a", "description": "A", "inputSchema": {"type": "object", "properties": {"z": {}, "y": {}}}}
    ]}});
    mcp_tools::bridge::sort_tools_by_name(&mut response);
    assert_eq!(
        response["result"]["tools"][0],
        json!({"name": "a", "description": "A", "inputSchema": {"type": "object", "properties": {"z": {}, "y": {}}}})
    );
    assert_eq!(response["result"]["tools"][1]["name"], "b");
}

#[tokio::test]
async fn bridge_relays_server_requests_to_the_client_during_tool_calls() {
    use futures_util::StreamExt;
    let addr = start_bridge(&[]).await;
    let url = format!("http://{addr}/mcp");
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {"roots": {}}, "clientInfo": {"name": "t", "version": "0"}}});
    let resp = http.post(&url).json(&init).send().await.unwrap();
    let sid = resp.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();

    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "roots", "arguments": {}}});
    let resp = http
        .post(&url)
        .header("mcp-session-id", &sid)
        .header("accept", "application/json, text/event-stream")
        .json(&call)
        .send()
        .await
        .unwrap();
    assert!(
        resp.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let mut events = resp.bytes_stream();
    let mut buf = String::new();
    let mut next_event = async || -> Value {
        loop {
            if let Some(end) = buf.find("\n\n") {
                let event: String = buf.drain(..end + 2).collect();
                let data: String = event
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect();
                if !data.is_empty() {
                    return serde_json::from_str(&data).unwrap();
                }
                continue;
            }
            buf.push_str(std::str::from_utf8(&events.next().await.unwrap().unwrap()).unwrap());
        }
    };
    let server_request = next_event().await;
    assert_eq!(server_request["method"], "roots/list");
    let answer = json!({"jsonrpc": "2.0", "id": server_request["id"], "result": {"roots": [{"uri": "file:///app"}]}});
    let ack = http
        .post(&url)
        .header("mcp-session-id", &sid)
        .json(&answer)
        .send()
        .await
        .unwrap();
    assert_eq!(ack.status(), 202);
    let final_response = next_event().await;
    assert_eq!(final_response["id"], 2);
    let text = final_response["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("file:///app"), "{text}");
}
