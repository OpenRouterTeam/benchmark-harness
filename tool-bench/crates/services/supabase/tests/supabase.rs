use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_supabase::Supabase;
use serde_json::{Value, json};
use tower::ServiceExt;

const PROD: &str = "qmvbxtzkplrnwdhyfaco";
const STAGING: &str = "hzrkcwpxnqdlmvbtejsa";
const MKT_OLD: &str = "wgtplxkqrzvnmbcdhfye";

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/cloud/supabase/top-customer-march-revenue");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8118")
        .header("authorization", "Bearer sbp_toolbench")
        .header("content-type", content_type)
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    (
        parts.status,
        parts.headers,
        to_bytes(body, usize::MAX).await.unwrap().to_vec(),
    )
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let (status, _, out) = raw(app, method, uri, "application/json", bytes).await;
    (status, serde_json::from_slice(&out).unwrap_or(Value::Null))
}

async fn sql(app: &Router, r: &str, query: &str) -> (StatusCode, Value) {
    call(
        app,
        "POST",
        &format!("/v1/projects/{r}/database/query"),
        json!({ "query": query }),
    )
    .await
}

#[tokio::test]
async fn requires_a_bearer_token() {
    let app = service_app(world(), &Supabase);
    let req = Request::get("/v1/projects").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn organizations_and_projects() {
    let app = service_app(world(), &Supabase);
    let (status, orgs) = call(&app, "GET", "/v1/organizations", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        orgs[0],
        json!({ "id": "northwind-labs", "slug": "northwind-labs", "name": "Northwind Labs" })
    );
    let (_, org) = call(&app, "GET", "/v1/organizations/northwind-labs", Value::Null).await;
    assert_eq!(org["plan"], "pro");
    let (status, err) = call(&app, "GET", "/v1/organizations/nope", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["message"], "Organization not found");

    let (_, projects) = call(&app, "GET", "/v1/projects", Value::Null).await;
    let names: Vec<&str> = projects
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "storefront-prod",
            "storefront-staging",
            "marketing-site",
            "marketing-site-legacy"
        ]
    );
    let (_, p) = call(&app, "GET", &format!("/v1/projects/{PROD}"), Value::Null).await;
    assert_eq!(p["status"], "ACTIVE_HEALTHY");
    let (status, _) = call(
        &app,
        "GET",
        "/v1/projects/zzzzzzzzzzzzzzzzzzzz",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn creates_projects_deterministically() {
    let w = world();
    let app = service_app(w.clone(), &Supabase);
    let body = json!({ "name": "analytics", "region": "eu-west-1", "organization_slug": "northwind-labs", "db_pass": "Sup3rSecretPw!" });
    let (status, p) = call(&app, "POST", "/v1/projects", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(p["status"], "COMING_UP");
    let r = p["ref"].as_str().unwrap();
    assert_eq!(r.len(), 20);
    assert!(r.chars().all(|c| c.is_ascii_lowercase()));
    let state = w.lock().unwrap().snapshot();
    assert_eq!(
        state["services"]["supabase"]["projects"][r]["status"],
        "ACTIVE_HEALTHY"
    );
    let (status, err) = call(
        &app,
        "POST",
        "/v1/projects",
        json!({ "name": "x", "organization_slug": "nope", "db_pass": "longenough" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["message"], "Organization not found");

    let other = service_app(world(), &Supabase);
    let (_, p2) = call(&other, "POST", "/v1/projects", body).await;
    assert_eq!(p2["ref"], p["ref"], "same scenario, same ref");
}

#[tokio::test]
async fn pause_and_restore() {
    let w = world();
    let app = service_app(w.clone(), &Supabase);
    let (status, _) = call(
        &app,
        "POST",
        &format!("/v1/projects/{MKT_OLD}/pause"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        w.lock().unwrap().snapshot()["services"]["supabase"]["projects"][MKT_OLD]["status"],
        "INACTIVE"
    );
    let (status, err) = call(
        &app,
        "POST",
        &format!("/v1/projects/{MKT_OLD}/pause"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["message"], "Project is already paused.");
    let (status, _) = sql(&app, MKT_OLD, "select 1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &app,
        "POST",
        &format!("/v1/projects/{MKT_OLD}/restore"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, err) = call(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/restore"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["message"], "Project is not paused.");
}

#[tokio::test]
async fn sql_reports_with_joins_and_aggregates() {
    let app = service_app(world(), &Supabase);
    let (status, rows) = sql(
        &app,
        PROD,
        "select c.full_name, c.email, sum(o.total) as revenue, count(*) as orders \
         from public.orders o join public.customers c on c.id = o.customer_id \
         where o.status in ('paid', 'shipped') and o.created_at >= '2026-03-01' and o.created_at < '2026-04-01' \
         group by c.id, c.full_name, c.email order by revenue desc limit 2",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        rows,
        json!([
            { "full_name": "Grace Kim", "email": "grace.kim@example.kr", "revenue": 1234.5, "orders": 3 },
            { "full_name": "Felix Wagner", "email": "felix.wagner@example.de", "revenue": 1100.0, "orders": 2 },
        ])
    );
    let (_, rows) = sql(
        &app,
        PROD,
        "select status, count(*) from orders group by status order by 2 desc, status",
    )
    .await;
    assert_eq!(rows[0], json!({ "status": "paid", "count": 8 }));
    let (_, rows) = sql(
        &app,
        PROD,
        "select date_trunc('month', created_at) as month, round(avg(total), 2) as avg_total from orders where status <> 'cancelled' group by 1 order by 1",
    )
    .await;
    assert_eq!(
        rows[0],
        json!({ "month": "2026-02-01T00:00:00+00:00", "avg_total": 787.55 })
    );
    let (_, rows) = sql(
        &app,
        PROD,
        "select id from customers where full_name ilike 'priya%' order by id offset 1",
    )
    .await;
    assert_eq!(rows, json!([{ "id": 1107 }, { "id": 1110 }]));
    let (_, rows) = sql(
        &app,
        PROD,
        "with spend as (select customer_id, sum(total) as t from orders where status = 'pending' group by customer_id) \
         select c.email, s.t from spend s left join customers c on c.id = s.customer_id where s.t > 100 order by s.t desc",
    )
    .await;
    assert_eq!(
        rows,
        json!([{ "email": "ben.okafor@example.com", "t": 1300.0 }, { "email": "priya.shah@acme.io", "t": 300.0 }, { "email": "priya.shah@fastmail.com", "t": 134.9 }])
    );
    let (_, rows) = sql(&app, PROD, "select count(*) as n from orders where customer_id = (select id from customers where email = 'grace.kim@example.kr')").await;
    assert_eq!(rows, json!([{ "n": 3 }]));
    let (_, rows) = sql(&app, STAGING, "select max(total) from orders").await;
    assert_eq!(rows, json!([{ "max": 9999.99 }]));
}

#[tokio::test]
async fn sql_errors_look_like_postgres() {
    let app = service_app(world(), &Supabase);
    for (query, message) in [
        (
            "select * from nope",
            "Failed to run sql query: ERROR:  42P01: relation \"nope\" does not exist\n",
        ),
        (
            "selec 1",
            "Failed to run sql query: ERROR:  42601: syntax error at or near \"selec\"\n",
        ),
        (
            "select missing from orders",
            "Failed to run sql query: ERROR:  42703: column \"missing\" does not exist\n",
        ),
        (
            "select id from orders o join customers c on c.id = o.customer_id",
            "Failed to run sql query: ERROR:  42702: column reference \"id\" is ambiguous\n",
        ),
        (
            "select customer_id, status, count(*) from orders group by customer_id",
            "Failed to run sql query: ERROR:  42803: column \"orders.status\" must appear in the GROUP BY clause or be used in an aggregate function\n",
        ),
        (
            "update orders set status = 'lost' where id = 5003",
            "Failed to run sql query: ERROR:  22P02: invalid input value for enum order_status: \"lost\"\n",
        ),
        (
            "delete from customers where id = 1007",
            "Failed to run sql query: ERROR:  23503: update or delete on table \"customers\" violates foreign key constraint \"orders_customer_id_fkey\" on table \"orders\"\n",
        ),
        (
            "insert into customers (id, email, full_name) values (1001, 'x@example.com', 'X')",
            "Failed to run sql query: ERROR:  23505: duplicate key value violates unique constraint \"customers_pkey\"\n",
        ),
    ] {
        let (status, body) = sql(&app, PROD, query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(body["message"], message, "{query}");
    }
    let (status, body) = call(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/database/query"),
        json!({ "query": "delete from orders", "read_only": true }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["message"],
        "Failed to run sql query: ERROR:  25006: cannot execute DELETE in a read-only transaction\n"
    );
}

#[tokio::test]
async fn sql_mutations_are_reflected_in_state_and_atomic() {
    let w = world();
    let app = service_app(w.clone(), &Supabase);
    let (status, rows) = sql(&app, PROD, "update orders set status = 'cancelled' where customer_id = 1042 and status = 'pending' returning id, status").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        rows,
        json!([{ "id": 5003, "status": "cancelled" }, { "id": 5011, "status": "cancelled" }])
    );
    let (_, rows) = sql(
        &app,
        PROD,
        "insert into customers (email, full_name, country) values ('new@example.com', 'New Person', 'IE') returning id, created_at",
    )
    .await;
    assert_eq!(
        rows,
        json!([{ "id": 1111, "created_at": "2026-04-02T09:00:14+00:00" }])
    );
    // A failing statement rolls back the whole script.
    let (status, _) = sql(
        &app,
        PROD,
        "update orders set total = 0 where id = 5001; select * from nope",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let state = w.lock().unwrap().snapshot();
    let db = &state["services"]["supabase"][format!("db.{PROD}.public.orders")];
    assert_eq!(db["5003"]["status"], "cancelled");
    assert_eq!(db["5007"]["status"], "paid");
    assert_eq!(db["5001"]["total"], 1500.0);
    assert_eq!(
        state["services"]["supabase"][format!("db.{PROD}.public.customers")]["1111"]["email"],
        "new@example.com"
    );
    let (_, rows) = sql(
        &app,
        PROD,
        "delete from invoices where order_id = 5017 returning number",
    )
    .await;
    assert_eq!(rows, json!([{ "number": "INV-2026-00012" }]));
}

#[tokio::test]
async fn introspection_queries_answer_list_tables_and_extensions() {
    let app = service_app(world(), &Supabase);
    // The server's list_tables SQL (abridged; recognised by its catalog references).
    let query = "with tables as (SELECT c.oid :: int8 AS id, nc.nspname AS schema, c.relrowsecurity AS rls_enabled FROM pg_namespace nc JOIN pg_class c ON nc.oid = c.relnamespace), columns as (select 1) select * from tables where schema in ($1)";
    let (status, rows) = call(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/database/query"),
        json!({ "query": query, "parameters": ["public"], "read_only": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let invoices = &rows[2];
    assert_eq!(invoices["name"], "invoices");
    assert_eq!(invoices["rls_enabled"], false);
    assert_eq!(invoices["live_rows_estimate"], 15);
    assert_eq!(
        invoices["primary_keys"],
        json!([{ "schema": "public", "table_name": "invoices", "name": "id", "table_id": 17003 }])
    );
    assert_eq!(invoices["relationships"][0]["target_table_name"], "orders");
    let status_col = &rows[1]["columns"][2];
    assert_eq!(status_col["data_type"], "USER-DEFINED");
    assert_eq!(
        status_col["enums"],
        json!(["pending", "paid", "shipped", "refunded", "cancelled"])
    );
    assert_eq!(status_col["id"], "17002.3");

    let ext_sql = "SELECT e.name, n.nspname AS schema FROM pg_available_extensions() e(name, default_version, comment) LEFT JOIN pg_extension x ON e.name = x.extname";
    let (_, exts) = sql(&app, PROD, ext_sql).await;
    assert_eq!(
        exts[0],
        json!({ "name": "plpgsql", "schema": "pg_catalog", "default_version": "1.0", "installed_version": "1.0", "comment": "PL/pgSQL procedural language" })
    );
    let (_, _) = sql(&app, PROD, "create extension if not exists pg_trgm").await;
    let (_, exts) = sql(&app, PROD, ext_sql).await;
    let trgm = exts
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "pg_trgm")
        .unwrap();
    assert_eq!(trgm["installed_version"], "1.6");
}

#[tokio::test]
async fn migrations_apply_ddl_and_feed_advisors() {
    let w = world();
    let app = service_app(w.clone(), &Supabase);
    let (_, lints) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/advisors/security"),
        Value::Null,
    )
    .await;
    let names: Vec<&str> = lints["lints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["rls_disabled_in_public"]);
    assert_eq!(lints["lints"][0]["metadata"]["name"], "invoices");

    let (status, _) = call(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/database/migrations"),
        json!({ "name": "enable_rls_on_invoices", "query": "alter table public.invoices enable row level security;" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, migrations) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/database/migrations"),
        Value::Null,
    )
    .await;
    assert_eq!(
        migrations.as_array().unwrap().last().unwrap(),
        &json!({ "version": "20260402090007", "name": "enable_rls_on_invoices" })
    );
    let (_, lints) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/advisors/security"),
        Value::Null,
    )
    .await;
    assert_eq!(lints["lints"][0]["name"], "rls_enabled_no_policy");
    let state = w.lock().unwrap().snapshot();
    assert_eq!(
        state["services"]["supabase"]["tables"][format!("{PROD}.public.invoices")]["rls_enabled"],
        true
    );

    let (status, body) = call(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/database/migrations"),
        json!({ "name": "bad", "query": "create table public.orders (id int)" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["message"],
        "Failed to run sql query: ERROR:  42P07: relation \"orders\" already exists\n"
    );

    let (_, _) = call(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/database/migrations"),
        json!({ "name": "add_refunds", "query": "create table public.refunds (id bigint generated always as identity primary key, order_id bigint not null references public.orders(id), reason text, created_at timestamptz default now()); create index on public.refunds (order_id);" }),
    )
    .await;
    let (_, rows) = sql(
        &app,
        PROD,
        "insert into refunds (order_id, reason) values (5006, 'damaged') returning *",
    )
    .await;
    assert_eq!(
        rows,
        json!([{ "id": 1, "order_id": 5006, "reason": "damaged", "created_at": "2026-04-02T09:00:28+00:00" }])
    );
    let (status, _) = sql(&app, PROD, "insert into refunds (order_id) values (999999)").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, perf) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/advisors/performance"),
        Value::Null,
    )
    .await;
    assert_eq!(perf["lints"][0]["name"], "auth_rls_initplan");
}

#[tokio::test]
async fn logs_endpoint_runs_clickhouse_style_sql() {
    let app = service_app(world(), &Supabase);
    let sql = "select id, timestamp, event_message, log_attributes['response.status_code'] as status_code from logs where source = 'edge_logs' order by timestamp desc limit 100";
    let uri = format!(
        "/v1/projects/{PROD}/analytics/endpoints/logs?sql={}&iso_timestamp_start=2026-10-29T12%3A00%3A00.000Z&iso_timestamp_end=2026-10-30T12%3A00%3A00.000Z",
        sql.replace(' ', "%20")
            .replace('\'', "%27")
            .replace('[', "%5B")
            .replace(']', "%5D")
    );
    let (status, body) = call(&app, "GET", &uri, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let rows = body["result"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        2,
        "wall-clock windows are shifted to the world's now"
    );
    assert_eq!(rows[0]["status_code"], 500);
    let (_, body) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/analytics/endpoints/logs?sql=select%20nope%20from%20logs"),
        Value::Null,
    )
    .await;
    assert_eq!(body["error"]["message"], "column \"nope\" does not exist");
}

#[tokio::test]
async fn keys_types_storage_and_secrets() {
    let app = service_app(world(), &Supabase);
    let (_, keys) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/api-keys?reveal=false"),
        Value::Null,
    )
    .await;
    let keys = keys.as_array().unwrap();
    assert_eq!(keys[0]["name"], "anon");
    assert!(keys[0]["api_key"].as_str().unwrap().starts_with("eyJ"));
    assert!(
        keys[2]["api_key"]
            .as_str()
            .unwrap()
            .starts_with("sb_publishable_")
    );
    assert!(
        keys[3]["api_key"]
            .as_str()
            .unwrap()
            .ends_with("\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}")
    );
    let (_, legacy) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/api-keys/legacy"),
        Value::Null,
    )
    .await;
    assert_eq!(legacy, json!({ "enabled": true }));

    let (_, types) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/types/typescript"),
        Value::Null,
    )
    .await;
    let ts = types["types"].as_str().unwrap();
    assert!(ts.contains("      orders: {\n        Row: {\n          id: number\n          customer_id: number\n          status: Database[\"public\"][\"Enums\"][\"order_status\"]"));
    assert!(ts.contains("pdf_path?: string | null"));

    let (_, buckets) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/storage/buckets"),
        Value::Null,
    )
    .await;
    assert_eq!(buckets[0]["name"], "invoices");
    assert_eq!(buckets[0]["public"], false);
    let (_, config) = call(
        &app,
        "GET",
        &format!("/v1/projects/{MKT_OLD}/config/storage"),
        Value::Null,
    )
    .await;
    assert_eq!(config["fileSizeLimit"], 52_428_800);
    let (status, err) = call(
        &app,
        "PATCH",
        &format!("/v1/projects/{MKT_OLD}/config/storage"),
        json!({ "fileSizeLimit": 104_857_600, "features": { "imageTransformation": { "enabled": false }, "s3Protocol": { "enabled": true } } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["message"].as_str().unwrap().contains("free plan"));
    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/v1/projects/{PROD}/config/storage"),
        json!({ "fileSizeLimit": 104_857_600, "features": { "imageTransformation": { "enabled": true }, "s3Protocol": { "enabled": false } } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, config) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/config/storage"),
        Value::Null,
    )
    .await;
    assert_eq!(config["features"]["s3Protocol"]["enabled"], false);

    let (status, _) = call(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/secrets"),
        json!([{ "name": "RESEND_API_KEY", "value": "re_123" }]),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, secrets) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/secrets"),
        Value::Null,
    )
    .await;
    assert_eq!(secrets[0]["name"], "RESEND_API_KEY");
    assert_ne!(secrets[0]["value"], "re_123");
}

#[tokio::test]
async fn edge_functions_list_body_and_deploy() {
    let w = world();
    let app = service_app(w.clone(), &Supabase);
    let (_, fns) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/functions"),
        Value::Null,
    )
    .await;
    assert_eq!(fns[0]["slug"], "stripe-webhook");
    assert_eq!(
        fns[0]["entrypoint_path"],
        format!(
            "file:///tmp/user_fn_{PROD}_4f1d2c3b-8a9e-4b7c-9d6e-1a2b3c4d5e6f_3/source/index.ts"
        )
    );
    let (status, _) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/functions/nope"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, headers, body) = raw(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/functions/send-order-email/body"),
        "application/json",
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers["content-type"].to_str().unwrap();
    let parts =
        mock_supabase::multipart::parse(&body, &mock_supabase::multipart::boundary(ct).unwrap());
    assert_eq!(parts[1].filename.as_deref(), Some("source/index.ts"));

    let boundary = "formdata-undici-0123";
    let upload = mock_supabase::multipart::encode(
        &[
            mock_supabase::multipart::Part {
                name: "metadata".into(),
                filename: Some("blob".into()),
                content_type: Some("application/json".into()),
                data: br#"{"name":"hello-world","entrypoint_path":"index.ts","verify_jwt":true}"#
                    .to_vec(),
            },
            mock_supabase::multipart::Part {
                name: "file".into(),
                filename: Some("index.ts".into()),
                content_type: Some("application/typescript".into()),
                data: b"Deno.serve(() => new Response('hello'));\n".to_vec(),
            },
        ],
        boundary,
    );
    let (status, _, out) = raw(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/functions/deploy?slug=hello-world"),
        &format!("multipart/form-data; boundary={boundary}"),
        upload.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let f: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(f["version"], 1);
    assert_eq!(f["status"], "ACTIVE");
    let (_, _, out) = raw(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/functions/deploy?slug=hello-world"),
        &format!("multipart/form-data; boundary={boundary}"),
        upload,
    )
    .await;
    let f2: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(f2["version"], 2);
    assert_eq!(f2["id"], f["id"]);
    let state = w.lock().unwrap().snapshot();
    assert_eq!(
        state["services"]["supabase"]["function_bodies"][format!("{PROD}/hello-world")]["files"][0]
            ["content"],
        "Deno.serve(() => new Response('hello'));\n"
    );
}

#[tokio::test]
async fn branches_create_merge_and_delete() {
    let w = world();
    let app = service_app(w.clone(), &Supabase);
    let (status, _) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/branches"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "branching disabled until the first branch"
    );
    let (status, branch) = call(
        &app,
        "POST",
        &format!("/v1/projects/{PROD}/branches"),
        json!({ "branch_name": "add-refunds" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let branch_ref = branch["project_ref"].as_str().unwrap().to_owned();
    let (_, list) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/branches"),
        Value::Null,
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["is_default"], true);
    let (_, rows) = sql(&app, &branch_ref, "select count(*) as n from orders").await;
    assert_eq!(
        rows,
        json!([{ "n": 0 }]),
        "branches copy the schema, not the data"
    );
    let (status, _) = call(
        &app,
        "POST",
        &format!("/v1/projects/{branch_ref}/database/migrations"),
        json!({ "name": "add_refund_reason", "query": "alter table public.orders add column refund_reason text;" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = branch["id"].as_str().unwrap();
    let (status, _) = call(&app, "POST", &format!("/v1/branches/{id}/merge"), json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, rows) = sql(
        &app,
        PROD,
        "select refund_reason from orders where id = 5001",
    )
    .await;
    assert_eq!(rows, json!([{ "refund_reason": null }]));
    let (status, _) = call(&app, "DELETE", &format!("/v1/branches/{id}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "GET",
        &format!("/v1/projects/{branch_ref}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, projects) = call(&app, "GET", "/v1/projects", Value::Null).await;
    assert_eq!(projects.as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn health_advisors_notebooks_and_profile() {
    let app = service_app(world(), &Supabase);
    let (status, body) = call(
        &app,
        "POST",
        &format!("/v2/projects/{PROD}/advisors/run"),
        json!({ "data": { "type": "project_advisors", "attributes": { "lints": [{ "name": "log_auth_error_rate_high" }] } } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["data"]["attributes"]["lints"][0]["name"],
        "log_auth_error_rate_high"
    );
    let (_, notebooks) = call(
        &app,
        "GET",
        &format!("/v2/projects/{PROD}/notebooks"),
        Value::Null,
    )
    .await;
    assert_eq!(notebooks, json!({ "data": [], "links": { "next": null } }));
    let (status, body) = call(
        &app,
        "GET",
        &format!("/v2/projects/{PROD}/notebooks/nope"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["message"], "Notebook not found");
    let (_, created) = call(
        &app,
        "POST",
        &format!("/v2/projects/{PROD}/notebooks"),
        json!({ "data": { "type": "notebook", "attributes": { "name": "Revenue", "cells": [{ "type": "markdown", "text": "# Revenue" }] } } }),
    )
    .await;
    let id = created["data"]["id"].as_str().unwrap();
    let (_, nb) = call(
        &app,
        "GET",
        &format!("/v2/projects/{PROD}/notebooks/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        nb["data"]["attributes"]["content"]["cells"][0]["id"],
        "cell-1"
    );

    let req = Request::get("/v1/profile")
        .header("authorization", "Bearer sbp_toolbench")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(resp.headers().contains_key("x-gotrue-id"));
}

#[tokio::test]
async fn content_api_serves_schema_and_search() {
    let app = service_app(world(), &Supabase);
    let req = Request::post("/docs/api/graphql")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"query":"{ schema }"}"#))
        .unwrap();
    let body: Value = serde_json::from_slice(
        &to_bytes(
            app.clone().oneshot(req).await.unwrap().into_body(),
            usize::MAX,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert!(
        body["data"]["schema"]
            .as_str()
            .unwrap()
            .contains("type Guide implements SearchResult")
    );

    let req = Request::get("/docs/api/graphql?query=%7B%20schema%20%7D")
        .body(Body::empty())
        .unwrap();
    let body: Value = serde_json::from_slice(
        &to_bytes(
            app.clone().oneshot(req).await.unwrap().into_body(),
            usize::MAX,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert!(
        body["data"]["schema"]
            .as_str()
            .unwrap()
            .starts_with("schema {")
    );

    let req = Request::post("/docs/api/graphql")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "query": "query { searchDocs(query: \"pause project\", limit: 1) { nodes { title href } } }" }).to_string()))
        .unwrap();
    let body: Value = serde_json::from_slice(
        &to_bytes(app.oneshot(req).await.unwrap().into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body["data"]["searchDocs"]["nodes"][0]["title"],
        "Pausing and restoring projects"
    );
}

#[tokio::test]
async fn unknown_routes_are_404s() {
    let app = service_app(world(), &Supabase);
    let (status, body) = call(&app, "GET", "/v1/nope", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "Not Found");
    let (status, _) = call(
        &app,
        "GET",
        &format!("/v1/projects/{PROD}/nope"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
