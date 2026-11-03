use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mock_cloudflare::Cloudflare;
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use serde_json::{Value, json};
use tower::ServiceExt;

const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
const ZONE: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const NS: &str = "c0ffee00c0ffee00c0ffee00c0ffee00";
const DB: &str = "5d1a0b7c-3e2f-4a9b-8c7d-6e5f4a3b2c1d";

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn setup() -> (SharedWorld, Router) {
    let w = world();
    let app = service_app(w.clone(), &Cloudflare);
    (w, app)
}

async fn tool(app: &Router, name: &str, args: Value) -> Value {
    let req = Request::post("/_mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "name": name, "arguments": args }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

/// Runs `code` with `execute` and parses its (JSON) text result.
async fn exec(app: &Router, code: &str) -> Value {
    let result = tool(app, "execute", json!({ "code": code })).await;
    assert!(result.get("isError").is_none(), "{}", text(&result));
    serde_json::from_str(text(&result)).unwrap_or_else(|_| Value::String(text(&result).to_owned()))
}

async fn exec_err(app: &Router, code: &str) -> String {
    let result = tool(app, "execute", json!({ "code": code })).await;
    assert_eq!(result["isError"], true, "{}", text(&result));
    text(&result).to_owned()
}

async fn rest(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    ctype: &str,
) -> (StatusCode, String, Vec<u8>) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", ctype)
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let ct = resp.headers()["content-type"].to_str().unwrap().to_owned();
    (
        status,
        ct,
        to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
}

async fn rest_json(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
    let (s, _, b) = rest(app, method, uri, body, "application/json").await;
    (s, serde_json::from_slice(&b).unwrap())
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["cloudflare"].clone()
}

// ── whoami / docs ────────────────────────────────────────────────────────────

#[tokio::test]
async fn whoami_returns_the_user_profile() {
    let (_, app) = setup();
    let r = tool(&app, "whoami", json!({})).await;
    let expected =
        json!({ "id": "user:7c5dae5552338874e5053f2534d2767a", "email": "ops@octo-corp.example" });
    assert_eq!(r["structuredContent"], expected);
    assert_eq!(text(&r), expected.to_string());
}

#[tokio::test]
async fn docs_returns_ranked_chunks_in_the_servers_format() {
    let (_, app) = setup();
    let r = tool(
        &app,
        "docs",
        json!({ "query": "how do I create a DNS record" }),
    )
    .await;
    let results = r["structuredContent"]["results"].as_array().unwrap();
    assert!(!results.is_empty() && results.len() <= 5);
    assert_eq!(
        results[0]["url"],
        "https://developers.cloudflare.com/dns/manage-dns-records/how-to/create-dns-records/"
    );
    assert_eq!(results[0]["title"], "Create Dns Records");
    assert!(text(&r).starts_with("<result>\n<url>https://developers.cloudflare.com/dns/"));
    let again = tool(
        &app,
        "docs",
        json!({ "query": "how do I create a DNS record" }),
    )
    .await;
    assert_eq!(r, again, "deterministic");
    let missing = tool(&app, "docs", json!({})).await;
    assert_eq!(missing["isError"], true);
}

// ── search ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn search_runs_code_against_the_pinned_spec() {
    let (_, app) = setup();
    let r = tool(
        &app,
        "search",
        json!({ "code": "async () => Object.keys(spec.paths).filter(p => p.includes('dns_records')).sort()" }),
    )
    .await;
    let paths: Vec<String> = serde_json::from_str(text(&r)).unwrap();
    assert!(paths.contains(&"/zones/{zone_id}/dns_records".to_owned()));
    assert!(paths.contains(&"/zones/{zone_id}/dns_records/{dns_record_id}".to_owned()));
    // Pretty-printed like the real formatter.
    assert!(text(&r).starts_with("[\n  \"/zones/"));

    let op = tool(
        &app,
        "search",
        json!({ "code": "async () => { const op = spec.paths['/zones/{zone_id}/dns_records']?.post; return { summary: op?.summary }; }" }),
    )
    .await;
    assert_eq!(text(&op), "{\n  \"summary\": \"Create DNS Record\"\n}");
}

#[tokio::test]
async fn search_has_no_network_and_reports_errors_like_the_server() {
    let (_, app) = setup();
    let r = tool(
        &app,
        "search",
        json!({ "code": "async () => cloudflare.request({ method: 'GET', path: '/zones' })" }),
    )
    .await;
    assert_eq!(r["isError"], true);
    assert_eq!(text(&r), "Error: cloudflare is not defined");

    let syntax = tool(
        &app,
        "search",
        json!({ "code": "async () => { return spec.paths[ }" }),
    )
    .await;
    assert_eq!(syntax["isError"], true);
    assert!(
        text(&syntax).starts_with("Error: Uncaught SyntaxError"),
        "{}",
        text(&syntax)
    );

    let thrown = tool(
        &app,
        "search",
        json!({ "code": "async () => { throw new Error('nope') }" }),
    )
    .await;
    assert_eq!(text(&thrown), "Error: nope");
}

#[tokio::test]
async fn runaway_code_hits_a_deterministic_cpu_limit() {
    let (_, app) = setup();
    let r = tool(
        &app,
        "search",
        json!({ "code": "async () => { while (true) {} }" }),
    )
    .await;
    assert_eq!(r["isError"], true);
    assert_eq!(text(&r), "Error: Worker exceeded CPU time limit.");
}

#[tokio::test]
async fn large_results_are_truncated_unless_truncate_is_false() {
    let (_, app) = setup();
    let code = "async () => Object.entries(spec.paths).map(([p, item]) => ({ p, item }))";
    let capped = tool(&app, "search", json!({ "code": code })).await;
    let t = text(&capped);
    assert!(t.len() <= 24_000, "{}", t.len());
    assert!(t.contains("--- TRUNCATED ---"));
    serde_json::from_str::<Value>(t).expect("structurally truncated JSON stays valid");
    let full = tool(&app, "search", json!({ "code": code, "truncate": false })).await;
    assert!(text(&full).len() > 100_000);
}

// ── execute ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn execute_calls_the_api_with_the_session_account() {
    let (w, app) = setup();
    let zones = exec(
        &app,
        "async () => { const r = await cloudflare.request({ method: 'GET', path: '/zones', query: { 'account.id': accountId } }); return { n: r.result.length, names: r.result.map(z => z.name), info: r.result_info, status: r.status }; }",
    )
    .await;
    assert_eq!(zones["names"], json!(["octo-corp.com", "octo-corp.dev"]));
    assert_eq!(zones["status"], 200);
    assert_eq!(
        zones["info"],
        json!({ "page": 1, "per_page": 20, "count": 2, "total_count": 2, "total_pages": 1 })
    );
    // Inner requests are journaled as API calls from the tools container.
    let journal = w.lock().unwrap().journal.entries().to_vec();
    let inner = journal
        .iter()
        .find(|e| e.path == "/client/v4/zones")
        .unwrap();
    assert_eq!(inner.method, "GET");
    assert_eq!(
        inner.query.as_deref(),
        Some("account.id=0123456789abcdef0123456789abcdef")
    );
    assert_eq!(inner.status, 200);
}

#[tokio::test]
async fn execute_creates_a_dns_record_with_a_stable_id() {
    let (w, app) = setup();
    let created = exec(
        &app,
        &format!("async () => (await cloudflare.request({{ method: 'POST', path: '/zones/{ZONE}/dns_records', body: {{ type: 'CNAME', name: 'status', content: 'Octo.StatusPage.io.', proxied: false, comment: 'status page' }} }})).result"),
    )
    .await;
    assert_eq!(created["name"], "status.octo-corp.com");
    assert_eq!(created["content"], "octo.statuspage.io");
    assert_eq!(created["proxiable"], true);
    assert_eq!(created["proxied"], false);
    assert_eq!(created["ttl"], 1);
    assert!(created.get("zone_id").is_none());
    let id = created["id"].as_str().unwrap();
    let s = state(&w);
    assert_eq!(s["dns_records"][id]["zone_id"], ZONE);
    assert_eq!(
        s["dns_records"][id]["created_on"],
        "2026-03-14T09:00:14.000000Z"
    );

    // The same record again is the real duplicate error, thrown with Cloudflare's codes.
    let err = exec_err(
        &app,
        &format!("async () => cloudflare.request({{ method: 'POST', path: '/zones/{ZONE}/dns_records', body: {{ type: 'CNAME', name: 'status.octo-corp.com', content: 'octo.statuspage.io' }} }})"),
    )
    .await;
    assert_eq!(
        err,
        format!(
            "Error: Cloudflare API error: POST /zones/{ZONE}/dns_records returned HTTP 400: 81058: An identical record already exists"
        )
    );

    // A fresh world gives the record the same id.
    let (_, app2) = setup();
    let again = exec(
        &app2,
        &format!("async () => (await cloudflare.request({{ method: 'POST', path: '/zones/{ZONE}/dns_records', body: {{ type: 'CNAME', name: 'status.octo-corp.com', content: 'octo.statuspage.io' }} }})).result.id"),
    )
    .await;
    assert_eq!(again, json!(id));
}

#[tokio::test]
async fn execute_errors_match_the_server() {
    let (_, app) = setup();
    let not_found = exec_err(
        &app,
        &format!("async () => cloudflare.request({{ method: 'GET', path: '/zones/{ZONE}/dns_records/0000' }})"),
    )
    .await;
    assert_eq!(
        not_found,
        format!(
            "Error: Cloudflare API error: GET /zones/{ZONE}/dns_records/0000 returned HTTP 404: 81044: Record does not exist"
        )
    );
    let bad_zone = exec_err(
        &app,
        "async () => cloudflare.request({ method: 'GET', path: '/zones/nope/dns_records' })",
    )
    .await;
    assert_eq!(
        bad_zone,
        "Error: Cloudflare API error: GET /zones/nope/dns_records returned HTTP 400: 7003: Could not route to /zones/nope/dns_records, perhaps your object identifier is invalid?, 7000: No route for that URI"
    );
    let a_record = exec_err(
        &app,
        &format!("async () => cloudflare.request({{ method: 'POST', path: '/zones/{ZONE}/dns_records', body: {{ type: 'A', name: 'x', content: 'not-an-ip' }} }})"),
    )
    .await;
    assert!(
        a_record.ends_with("returned HTTP 400: 1004: DNS Validation Error"),
        "{a_record}"
    );
    // Caught errors are the program's business.
    let caught = exec(
        &app,
        "async () => { try { await cloudflare.request({ method: 'GET', path: '/zones/nope' }); } catch (e) { return 'caught: ' + e.message.slice(0, 30); } }",
    )
    .await;
    assert_eq!(caught, json!("caught: Cloudflare API error: GET /zon"));
    let undefined = tool(&app, "execute", json!({ "code": "async () => {}" })).await;
    assert_eq!(text(&undefined), "undefined");
}

#[tokio::test]
async fn execute_needs_account_id_with_several_accounts() {
    let (w, app) = setup();
    w.lock()
        .unwrap()
        .service_mut("cloudflare")
        .store
        .collection_mut("accounts")
        .insert(
            "fedcba9876543210fedcba9876543210",
            json!({ "id": "fedcba9876543210fedcba9876543210", "name": "Octo Corp Staging" }),
        );
    let err = exec_err(&app, "async () => accountId").await;
    assert_eq!(
        err,
        "Error: No account selected. Pass account_id with one of this session's accounts:\n- 0123456789abcdef0123456789abcdef (Octo Corp)\n- fedcba9876543210fedcba9876543210 (Octo Corp Staging)"
    );
    let ok = tool(&app, "execute", json!({ "code": "async () => accountId", "account_id": "fedcba9876543210fedcba9876543210" })).await;
    assert_eq!(text(&ok), "fedcba9876543210fedcba9876543210");
    let accounts = exec(&app, "async () => (await cloudflare.request({ method: 'GET', path: '/accounts', query: { name: 'Octo Corp Staging' } })).result.map(a => a.id)").await;
    assert_eq!(accounts, json!(["fedcba9876543210fedcba9876543210"]));

    let unknown = tool(
        &app,
        "execute",
        json!({ "code": "async () => cloudflare.request({ method: 'GET', path: `/accounts/${accountId}/workers/scripts` })", "account_id": "11111111111111111111111111111111" }),
    )
    .await;
    assert_eq!(
        text(&unknown),
        "Error: Cloudflare API error: GET /accounts/11111111111111111111111111111111/workers/scripts returned HTTP 403: 9109: Unauthorized to access requested resource\n\naccount_id 11111111111111111111111111111111 is not one of this session's accounts:\n- 0123456789abcdef0123456789abcdef (Octo Corp)\n- fedcba9876543210fedcba9876543210 (Octo Corp Staging)"
    );
}

// ── REST over HTTP ───────────────────────────────────────────────────────────

#[tokio::test]
async fn zones_list_filters_and_pages() {
    let (_, app) = setup();
    let (s, body) = rest_json(&app, "GET", "/client/v4/zones?name=octo-corp.dev", "").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["result"][0]["id"], "b2c3d4e5f60718293a4b5c6d7e8f90a1");
    assert_eq!(body["result"][0]["plan"]["name"], "Free Website");
    let (_, page2) = rest_json(&app, "GET", "/client/v4/zones?per_page=1&page=2", "").await;
    assert_eq!(page2["result"][0]["name"], "octo-corp.dev");
    assert_eq!(
        page2["result_info"],
        json!({ "page": 2, "per_page": 1, "count": 1, "total_count": 2, "total_pages": 2 })
    );
    let (_, contains) = rest_json(&app, "GET", "/client/v4/zones?name=contains:corp.c", "").await;
    assert_eq!(contains["result"].as_array().unwrap().len(), 1);
    let (s, missing) = rest_json(
        &app,
        "GET",
        "/client/v4/zones/ffffffffffffffffffffffffffffffff",
        "",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(missing["errors"][0]["code"], 7003);
    assert_eq!(missing["success"], false);
    assert_eq!(missing["result"], Value::Null);
    let (s, unrouted) = rest_json(&app, "GET", "/client/v4/nothing/here", "").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(
        unrouted["errors"],
        json!([{ "code": 7000, "message": "No route for that URI" }])
    );
}

#[tokio::test]
async fn zone_create_patch_delete() {
    let (w, app) = setup();
    let (_, created) = rest_json(
        &app,
        "POST",
        "/client/v4/zones",
        &json!({ "name": "octo-shop.io", "account": { "id": ACCOUNT } }).to_string(),
    )
    .await;
    assert_eq!(created["result"]["status"], "pending");
    let id = created["result"]["id"].as_str().unwrap().to_owned();
    let (s, dup) = rest_json(
        &app,
        "POST",
        "/client/v4/zones",
        &json!({ "name": "octo-shop.io", "account": { "id": ACCOUNT } }).to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(dup["errors"][0]["code"], 1061);
    let (_, paused) = rest_json(
        &app,
        "PATCH",
        &format!("/client/v4/zones/{id}"),
        r#"{"paused":true}"#,
    )
    .await;
    assert_eq!(paused["result"]["paused"], true);
    let (_, deleted) = rest_json(&app, "DELETE", &format!("/client/v4/zones/{ZONE}"), "").await;
    assert_eq!(deleted["result"], json!({ "id": ZONE }));
    let s = state(&w);
    assert!(s["zones"].get(ZONE).is_none());
    assert!(
        s["dns_records"]
            .get("3a1f0c9e2b7d4e6f8a9b0c1d2e3f4a5b")
            .is_none()
    );
    assert!(
        s["workers_routes"]
            .get("f1e2d3c4b5a697887766554433221100")
            .is_none()
    );
}

#[tokio::test]
async fn dns_record_filters_edit_batch_and_export() {
    let (w, app) = setup();
    let base = format!("/client/v4/zones/{ZONE}/dns_records");
    let (_, proxied) = rest_json(&app, "GET", &format!("{base}?proxied=true&type=A"), "").await;
    assert_eq!(proxied["result"].as_array().unwrap().len(), 1);
    assert_eq!(proxied["result"][0]["content"], "203.0.113.10");
    let (_, contains) = rest_json(&app, "GET", &format!("{base}?name.contains=www"), "").await;
    assert_eq!(contains["result"][0]["type"], "CNAME");
    let (_, any) = rest_json(
        &app,
        "GET",
        &format!("{base}?type=MX&tag=team:web&match=any"),
        "",
    )
    .await;
    assert_eq!(any["result"].as_array().unwrap().len(), 2);
    let (_, mx) = rest_json(&app, "GET", &format!("{base}?type=MX"), "").await;
    assert_eq!(mx["result"][0]["priority"], 10);

    let rec = "3a1f0c9e2b7d4e6f8a9b0c1d2e3f4a5b";
    let (_, patched) = rest_json(
        &app,
        "PATCH",
        &format!("{base}/{rec}"),
        r#"{"content":"203.0.113.99"}"#,
    )
    .await;
    assert_eq!(patched["result"]["id"], rec);
    assert_eq!(patched["result"]["content"], "203.0.113.99");
    assert_eq!(patched["result"]["proxied"], true);
    assert_eq!(state(&w)["dns_records"][rec]["content"], "203.0.113.99");

    let (s, conflict) = rest_json(
        &app,
        "POST",
        &base,
        r#"{"type":"A","name":"www","content":"192.0.2.1"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(conflict["errors"][0]["code"], 81053);
    let (s, txt) = rest_json(
        &app,
        "POST",
        &base,
        r#"{"type":"TXT","name":"_x","content":"v=1","proxied":true}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(txt["errors"][0]["error_chain"][0]["code"], 9004);

    // A failing batch changes nothing.
    let (s, _) = rest_json(&app, "POST", &format!("{base}/batch"), &json!({ "deletes": [{ "id": rec }], "posts": [{ "type": "A", "name": "bad", "content": "x" }] }).to_string()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(state(&w)["dns_records"].get(rec).is_some());
    let (_, batch) = rest_json(&app, "POST", &format!("{base}/batch"), &json!({ "deletes": [{ "id": rec }], "posts": [{ "type": "A", "name": "@", "content": "203.0.113.11", "proxied": true }] }).to_string()).await;
    assert_eq!(batch["result"]["deletes"][0]["id"], rec);
    assert_eq!(batch["result"]["posts"][0]["name"], "octo-corp.com");

    let (s, ct, export) = rest(&app, "GET", &format!("{base}/export"), "", "text/plain").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ct, "text/plain");
    let export = String::from_utf8(export).unwrap();
    assert!(
        export
            .contains("www.octo-corp.com.\t1\tIN\tCNAME\tocto-corp.com. ; cf_tags=cf-proxied:true"),
        "{export}"
    );
    assert!(export.contains("octo-corp.com.\t3600\tIN\tMX\t10 mx1.mail.example.net."));

    let (_, deleted) = rest_json(
        &app,
        "DELETE",
        &format!("{base}/4b2a1d0f3c8e5f7a9b0c1d2e3f4a5b6c"),
        "",
    )
    .await;
    assert_eq!(deleted["result"]["id"], "4b2a1d0f3c8e5f7a9b0c1d2e3f4a5b6c");
    let (s, gone) = rest_json(
        &app,
        "GET",
        &format!("{base}/4b2a1d0f3c8e5f7a9b0c1d2e3f4a5b6c"),
        "",
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(
        gone["errors"],
        json!([{ "code": 81044, "message": "Record does not exist." }])
    );
}

#[tokio::test]
async fn cache_purge_requires_a_target() {
    let (w, app) = setup();
    let (s, _) = rest_json(
        &app,
        "POST",
        &format!("/client/v4/zones/{ZONE}/purge_cache"),
        "{}",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, ok) = rest_json(
        &app,
        "POST",
        &format!("/client/v4/zones/{ZONE}/purge_cache"),
        r#"{"files":["https://octo-corp.com/app.js"]}"#,
    )
    .await;
    assert_eq!(ok["result"], json!({ "id": ZONE }));
    assert_eq!(
        state(&w)["cache_purges"][format!("{ZONE}#1")]["request"]["files"][0],
        "https://octo-corp.com/app.js"
    );
}

// ── Workers ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn worker_upload_with_form_data_settings_and_secrets() {
    let (w, app) = setup();
    let code = r#"async () => {
  const code = `export default { async fetch(request, env) { return new Response(await env.MY_KV.get('key') || 'none') } };`;
  const metadata = { main_module: "worker.js", compatibility_date: "2026-01-12", bindings: [{ type: "kv_namespace", name: "MY_KV", namespace_id: "c0ffee00c0ffee00c0ffee00c0ffee00" }] };
  const form = new FormData();
  form.append("metadata", new Blob([JSON.stringify(metadata)], { type: "application/json" }));
  form.append("worker.js", new Blob([code], { type: "application/javascript+module" }), "worker.js");
  return cloudflare.request({ method: "PUT", path: `/accounts/${accountId}/workers/scripts/my-worker`, body: form });
}"#;
    let r = exec(&app, code).await;
    assert_eq!(r["success"], true);
    assert_eq!(r["result"]["id"], "my-worker");
    assert_eq!(r["result"]["handlers"], json!(["fetch"]));
    let s = state(&w);
    let doc = &s["workers_scripts"][format!("{ACCOUNT}/my-worker")];
    assert!(doc["content"].as_str().unwrap().contains("env.MY_KV.get"));
    assert_eq!(doc["bindings"][0]["name"], "MY_KV");

    let content = exec(&app, "async () => (await cloudflare.request({ method: 'GET', path: `/accounts/${accountId}/workers/scripts/my-worker` })).result").await;
    assert!(content.as_str().unwrap().starts_with("export default"));
    let list = exec(&app, "async () => (await cloudflare.request({ method: 'GET', path: `/accounts/${accountId}/workers/scripts` })).result.map(s => s.id)").await;
    assert_eq!(list, json!(["api-gateway", "my-worker"]));

    let secret = exec(&app, "async () => (await cloudflare.request({ method: 'PUT', path: `/accounts/${accountId}/workers/scripts/api-gateway/secrets`, body: { name: 'DB_URL', text: 'postgres://x', type: 'secret_text' } })).result").await;
    assert_eq!(secret, json!({ "name": "DB_URL", "type": "secret_text" }));
    let settings = exec(&app, "async () => (await cloudflare.request({ method: 'GET', path: `/accounts/${accountId}/workers/scripts/api-gateway/settings` })).result.bindings.map(b => b.name)").await;
    assert_eq!(settings, json!(["CONFIG", "UPSTREAM_TOKEN", "DB_URL"]));
    let missing = exec_err(&app, "async () => cloudflare.request({ method: 'GET', path: `/accounts/${accountId}/workers/scripts/nope/settings` })").await;
    assert!(
        missing.ends_with("returned HTTP 404: 10007: This Worker does not exist on your account"),
        "{missing}"
    );
}

#[tokio::test]
async fn worker_routes_validate_zone_and_script() {
    let (w, app) = setup();
    let base = format!("/client/v4/zones/{ZONE}/workers/routes");
    let (_, created) = rest_json(
        &app,
        "POST",
        &base,
        r#"{"pattern":"api.octo-corp.com/v2/*","script":"api-gateway"}"#,
    )
    .await;
    let id = created["result"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["result"]["pattern"], "api.octo-corp.com/v2/*");
    assert_eq!(state(&w)["workers_routes"][&id]["script"], "api-gateway");
    let (s, dup) = rest_json(
        &app,
        "POST",
        &base,
        r#"{"pattern":"api.octo-corp.com/v2/*","script":"api-gateway"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(dup["errors"][0]["code"], 10020);
    let (s, other_zone) = rest_json(
        &app,
        "POST",
        &base,
        r#"{"pattern":"api.octo-corp.dev/*","script":"api-gateway"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(other_zone["errors"][0]["code"], 10022);
    let (s, no_script) = rest_json(
        &app,
        "POST",
        &base,
        r#"{"pattern":"octo-corp.com/x/*","script":"ghost"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(no_script["errors"][0]["code"], 10007);
    let (_, list) = rest_json(&app, "GET", &base, "").await;
    assert_eq!(list["result"].as_array().unwrap().len(), 2);
    let (_, updated) = rest_json(
        &app,
        "PUT",
        &format!("{base}/{id}"),
        r#"{"pattern":"api.octo-corp.com/v3/*","script":"api-gateway"}"#,
    )
    .await;
    assert_eq!(updated["result"]["id"], id);
    let (_, deleted) = rest_json(&app, "DELETE", &format!("{base}/{id}"), "").await;
    assert_eq!(deleted["result"], json!({ "id": id }));
}

// ── KV ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn kv_values_read_write_list_and_bulk() {
    let (w, app) = setup();
    let ns = format!("/accounts/${{accountId}}/storage/kv/namespaces/{NS}");
    let banner = exec(&app, &format!("async () => (await cloudflare.request({{ method: 'GET', path: `{ns}/values/banner` }})).result")).await;
    assert_eq!(banner, json!("Spring sale ends Friday"));
    let written = exec(
        &app,
        &format!("async () => cloudflare.request({{ method: 'PUT', path: `{ns}/values/flags`, body: {{ checkout_v2: true, rollout: 25 }} }})"),
    )
    .await;
    assert_eq!(written["success"], true);
    let s = state(&w);
    let doc = &s["kv_values"][format!("{NS}/flags")];
    assert_eq!(doc["value"], "{\"checkout_v2\":true,\"rollout\":25}");
    assert_eq!(
        doc["value_json"],
        json!({ "checkout_v2": true, "rollout": 25 })
    );

    let text_value = exec(&app, &format!("async () => cloudflare.request({{ method: 'PUT', path: `{ns}/values/motd`, body: 'hello there', contentType: 'text/plain' }})")).await;
    assert_eq!(text_value["success"], true);
    assert_eq!(
        state(&w)["kv_values"][format!("{NS}/motd")]["value"],
        "hello there"
    );

    let (_, keys) = rest_json(
        &app,
        "GET",
        &format!("/client/v4/accounts/{ACCOUNT}/storage/kv/namespaces/{NS}/keys?limit=10"),
        "",
    )
    .await;
    let names: Vec<&str> = keys["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["banner", "flags", "motd"]);
    assert_eq!(keys["result_info"], json!({ "count": 3, "cursor": "" }));
    let (_, prefixed) = rest_json(
        &app,
        "GET",
        &format!("/client/v4/accounts/{ACCOUNT}/storage/kv/namespaces/{NS}/keys?prefix=fl"),
        "",
    )
    .await;
    assert_eq!(prefixed["result"][0]["metadata"], Value::Null);

    let missing = exec_err(
        &app,
        &format!("async () => cloudflare.request({{ method: 'GET', path: `{ns}/values/nope` }})"),
    )
    .await;
    assert!(
        missing.ends_with("returned HTTP 404: 10009: get: 'key not found'"),
        "{missing}"
    );

    let (_, bulk) = rest_json(
        &app,
        "PUT",
        &format!("/client/v4/accounts/{ACCOUNT}/storage/kv/namespaces/{NS}/bulk"),
        r#"[{"key":"a","value":"1"},{"key":"b","value":"2","expiration_ttl":3600}]"#,
    )
    .await;
    assert_eq!(bulk["result"]["successful_key_count"], 2);
    let (_, got) = rest_json(
        &app,
        "POST",
        &format!("/client/v4/accounts/{ACCOUNT}/storage/kv/namespaces/{NS}/bulk/get"),
        r#"{"keys":["a","zzz"]}"#,
    )
    .await;
    assert_eq!(got["result"]["values"], json!({ "a": "1", "zzz": null }));
    let (_, del) = rest_json(
        &app,
        "POST",
        &format!("/client/v4/accounts/{ACCOUNT}/storage/kv/namespaces/{NS}/bulk/delete"),
        r#"["a","b"]"#,
    )
    .await;
    assert_eq!(del["result"]["successful_key_count"], 2);
    assert!(state(&w)["kv_values"].get(format!("{NS}/a")).is_none());

    let (_, namespaces) = rest_json(
        &app,
        "GET",
        &format!("/client/v4/accounts/{ACCOUNT}/storage/kv/namespaces"),
        "",
    )
    .await;
    assert_eq!(
        namespaces["result"][0],
        json!({ "id": NS, "title": "web-config", "supports_url_encoding": true, "beta": false })
    );
    let (s, dup) = rest_json(
        &app,
        "POST",
        &format!("/client/v4/accounts/{ACCOUNT}/storage/kv/namespaces"),
        r#"{"title":"web-config"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(dup["errors"][0]["code"], 10014);
}

// ── R2 / D1 / members ────────────────────────────────────────────────────────

#[tokio::test]
async fn r2_buckets_list_create_and_delete() {
    let (w, app) = setup();
    let base = format!("/client/v4/accounts/{ACCOUNT}/r2/buckets");
    let (_, list) = rest_json(&app, "GET", &base, "").await;
    assert_eq!(list["result"]["buckets"][0]["name"], "assets-prod");
    let (_, created) = rest_json(
        &app,
        "POST",
        &base,
        r#"{"name":"logs-archive","locationHint":"WEUR"}"#,
    )
    .await;
    assert_eq!(created["result"]["location"], "weur");
    let (s, bad) = rest_json(&app, "POST", &base, r#"{"name":"Bad_Name"}"#).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(bad["errors"][0]["code"], 10005);
    let (s, busy) = rest_json(&app, "DELETE", &format!("{base}/assets-prod"), "").await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(busy["errors"][0]["code"], 10008);
    let (s, _) = rest_json(&app, "DELETE", &format!("{base}/tmp-exports"), "").await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        state(&w)["r2_buckets"]
            .get(format!("{ACCOUNT}/tmp-exports"))
            .is_none()
    );
    let (s, gone) = rest_json(&app, "GET", &format!("{base}/tmp-exports"), "").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(gone["errors"][0]["code"], 10006);
}

#[tokio::test]
async fn d1_runs_sqlite_and_persists_writes() {
    let (w, app) = setup();
    let q = format!("/client/v4/accounts/{ACCOUNT}/d1/database/{DB}/query");
    let (_, report) = rest_json(&app, "POST", &q, r#"{"sql":"SELECT customer, SUM(total_cents) AS cents FROM orders WHERE status = ? GROUP BY customer ORDER BY cents DESC","params":["paid"]}"#).await;
    assert_eq!(
        report["result"][0]["results"],
        json!([{ "customer": "ada@example.com", "cents": 5700 }])
    );
    assert_eq!(report["result"][0]["meta"]["changed_db"], false);

    let (_, insert) = rest_json(&app, "POST", &q, r#"{"sql":"INSERT INTO orders (customer, total_cents, status) VALUES ('cy@example.com', 900, 'paid') RETURNING id"}"#).await;
    assert_eq!(insert["result"][0]["results"], json!([{ "id": 4 }]));
    assert_eq!(insert["result"][0]["meta"]["changes"], 1);
    let rows = state(&w)["d1_tables"][format!("{DB}/orders")]["rows"].clone();
    assert_eq!(rows.as_array().unwrap().len(), 4);
    assert_eq!(
        rows[3],
        json!({ "id": 4, "customer": "cy@example.com", "total_cents": 900, "status": "paid" })
    );

    let (_, raw) = rest_json(
        &app,
        "POST",
        &format!("/client/v4/accounts/{ACCOUNT}/d1/database/{DB}/raw"),
        r#"{"sql":"SELECT COUNT(*) AS n FROM orders"}"#,
    )
    .await;
    assert_eq!(
        raw["result"][0]["results"],
        json!({ "columns": ["n"], "rows": [[4]] })
    );

    let (s, err) = rest_json(&app, "POST", &q, r#"{"sql":"SELEC 1"}"#).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(err["errors"][0]["code"], 7500);
    assert!(
        err["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("syntax error")
    );

    let (_, created) = rest_json(
        &app,
        "POST",
        &format!("/client/v4/accounts/{ACCOUNT}/d1/database"),
        r#"{"name":"analytics"}"#,
    )
    .await;
    let uuid = created["result"]["uuid"].as_str().unwrap().to_owned();
    let (_, ddl) = rest_json(&app, "POST", &format!("/client/v4/accounts/{ACCOUNT}/d1/database/{uuid}/query"), r#"{"sql":"CREATE TABLE events (id INTEGER PRIMARY KEY, kind TEXT); CREATE INDEX events_kind ON events(kind); INSERT INTO events (kind) VALUES ('signup');"}"#).await;
    assert_eq!(ddl["result"].as_array().unwrap().len(), 3);
    let (_, again) = rest_json(
        &app,
        "POST",
        &format!("/client/v4/accounts/{ACCOUNT}/d1/database/{uuid}/query"),
        r#"{"sql":"SELECT kind FROM events"}"#,
    )
    .await;
    assert_eq!(again["result"][0]["results"], json!([{ "kind": "signup" }]));
    let (_, list) = rest_json(
        &app,
        "GET",
        &format!("/client/v4/accounts/{ACCOUNT}/d1/database"),
        "",
    )
    .await;
    assert_eq!(list["result"].as_array().unwrap().len(), 2);
    let (s, missing) = rest_json(
        &app,
        "GET",
        &format!("/client/v4/accounts/{ACCOUNT}/d1/database/00000000-0000-4000-a000-000000000000"),
        "",
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(missing["errors"][0]["code"], 7404);
}

#[tokio::test]
async fn account_members_and_identity() {
    let (w, app) = setup();
    let base = format!("/client/v4/accounts/{ACCOUNT}/members");
    let (_, added) = rest_json(
        &app,
        "POST",
        &base,
        r#"{"email":"Contractor@Example.com","roles":["3536bcfad5faccb999b47003c79917fb"]}"#,
    )
    .await;
    assert_eq!(added["result"]["status"], "pending");
    assert_eq!(added["result"]["roles"][0]["name"], "DNS");
    let id = added["result"]["id"].as_str().unwrap().to_owned();
    let (_, list) = rest_json(&app, "GET", &base, "").await;
    assert_eq!(list["result"].as_array().unwrap().len(), 2);
    let (_, removed) = rest_json(&app, "DELETE", &format!("{base}/{id}"), "").await;
    assert_eq!(removed["result"]["id"], id);
    assert!(state(&w)["members"].get(&id).is_none());

    let (_, user) = rest_json(&app, "GET", "/client/v4/user", "").await;
    assert_eq!(user["result"]["email"], "ops@octo-corp.example");
    let (_, verify) = rest_json(&app, "GET", "/client/v4/user/tokens/verify", "").await;
    assert_eq!(verify["result"]["status"], "active");
    let (_, memberships) = rest_json(&app, "GET", "/client/v4/memberships", "").await;
    assert_eq!(memberships["result"][0]["account"]["name"], "Octo Corp");
    let (s, denied) = rest_json(
        &app,
        "GET",
        "/client/v4/accounts/ffffffffffffffffffffffffffffffff/members",
        "",
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(denied["errors"][0]["code"], 9109);
}
