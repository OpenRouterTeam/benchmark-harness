use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{WorldState, shared},
};
use mock_elasticsearch::Elasticsearch;
use serde_json::{Value, json};
use tower::ServiceExt;

// Fixture (now = 2026-10-30T14:00:00Z): checkout logs (prod 10.29/10.30, staging 10.30),
// a metrics index, and orders-2026 / orders-2025 (closed).
fn app() -> Router {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    let world = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    service_app(world, &Elasticsearch)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    auth: bool,
) -> (StatusCode, HeaderMap, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8106")
        .header("content-type", "application/json");
    if auth {
        req = req.header("authorization", "ApiKey es-toolbench");
    }
    let body = body.map_or(Body::empty(), |b| Body::from(b.to_string()));
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    (
        parts.status,
        parts.headers,
        String::from_utf8(bytes.to_vec()).unwrap(),
    )
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let (s, _, t) = send(app, "GET", uri, None, true).await;
    (s, serde_json::from_str(&t).unwrap_or(Value::Null))
}

async fn search(app: &Router, index: &str, body: Value) -> (StatusCode, Value) {
    let (s, _, t) = send(app, "POST", &format!("/{index}/_search"), Some(body), true).await;
    (s, serde_json::from_str(&t).unwrap_or(Value::Null))
}

async fn esql(app: &Router, query: &str) -> (StatusCode, Value) {
    let (s, _, t) = send(
        app,
        "POST",
        "/_query",
        Some(json!({ "query": query })),
        true,
    )
    .await;
    (s, serde_json::from_str(&t).unwrap_or(Value::Null))
}

fn ids(v: &Value) -> Vec<String> {
    v["hits"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["_id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn root_requires_auth_and_sends_product_header() {
    let app = app();
    let (status, headers, text) = send(&app, "GET", "/", None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers["x-elastic-product"], "Elasticsearch");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["error"]["type"], "security_exception");
    assert_eq!(body["error"]["root_cause"][0]["type"], "security_exception");
    assert_eq!(body["status"], 401);

    let (status, headers, text) = send(&app, "GET", "/", None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-elastic-product"], "Elasticsearch");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["version"]["number"], "9.1.5");
    assert_eq!(body["tagline"], "You Know, for Search");
}

#[tokio::test]
async fn cat_indices_json_text_and_patterns() {
    let app = app();
    let (status, body) = get(
        &app,
        "/_cat/indices/logs-checkout-*?h=index,status,docs.count&format=json",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([
            {"index": "logs-checkout-prod-2026.10.29", "status": "open", "docs.count": "12"},
            {"index": "logs-checkout-prod-2026.10.30", "status": "open", "docs.count": "39"},
            {"index": "logs-checkout-staging-2026.10.30", "status": "open", "docs.count": "15"}
        ])
    );
    let (_, body) = get(&app, "/_cat/indices/logs-checkout-prod?format=json&h=index").await;
    assert_eq!(
        body,
        json!([{"index": "logs-checkout-prod-2026.10.30"}]),
        "alias resolves"
    );
    let (_, body) = get(
        &app,
        "/_cat/indices?format=json&h=index,health&health=yellow",
    )
    .await;
    assert_eq!(
        body,
        json!([{"index": "logs-checkout-staging-2026.10.30", "health": "yellow"}])
    );
    let (_, _, text) = send(
        &app,
        "GET",
        "/_cat/indices/orders-*?v&h=index,status&s=index",
        None,
        true,
    )
    .await;
    assert_eq!(
        text,
        "index       status\norders-2025 close\norders-2026 open\n"
    );
    let (status, body) = get(&app, "/_cat/indices/nope?format=json").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["type"], "index_not_found_exception");
    assert_eq!(body["error"]["reason"], "no such index [nope]");
    let (_, body) = get(&app, "/_cat/indices/nope-*?format=json").await;
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn cat_shards_lists_primaries_and_replicas() {
    let app = app();
    let (_, body) = get(
        &app,
        "/_cat/shards/orders-2026?format=json&h=index,shard,prirep,state,docs,store,node",
    )
    .await;
    let shards = body.as_array().unwrap();
    assert_eq!(shards.len(), 6, "3 primaries + 3 replicas");
    assert_eq!(shards[0]["prirep"], "p");
    assert_eq!(shards[0]["docs"], "4");
    let (_, body) = get(
        &app,
        "/_cat/shards/logs-checkout-staging-2026.10.30?format=json",
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 1, "rep 0");
    let (_, body) = get(&app, "/_cat/shards?format=json").await;
    assert_eq!(body.as_array().unwrap().len(), 2 + 2 + 1 + 2 + 6 + 2);
}

#[tokio::test]
async fn mappings_and_missing_index() {
    let app = app();
    let (status, body) = get(&app, "/orders-2026/_mapping").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["orders-2026"]["mappings"]["properties"]["order_id"]["type"],
        "keyword"
    );
    let (_, body) = get(&app, "/logs-checkout-prod-*/_mapping").await;
    assert_eq!(body.as_object().unwrap().len(), 2);
    let (status, body) = get(&app, "/nope/_mapping").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["index"], "nope");
}

#[tokio::test]
async fn match_term_terms_and_bool() {
    let app = app();
    let (_, body) = search(
        &app,
        "logs-checkout-prod-2026.10.30",
        json!({"query": {"match": {"message": "redis refused"}}}),
    )
    .await;
    assert_eq!(body["hits"]["total"], json!({"value": 3, "relation": "eq"}));
    assert_eq!(body["hits"]["hits"][0]["_source"]["service"], "cart-api");
    assert!(body["hits"]["max_score"].as_f64().unwrap() > 0.0);

    let (_, body) = search(
        &app,
        "logs-checkout-prod-2026.10.30",
        json!({"query": {"match": {"message": {"query": "redis deadlock", "operator": "and"}}}}),
    )
    .await;
    assert_eq!(body["hits"]["total"]["value"], 0);

    let (_, body) = search(
        &app,
        "orders-2026",
        json!({"query": {"term": {"customer_email": "jordan.lee@example.com"}}, "size": 0}),
    )
    .await;
    assert_eq!(body["hits"]["total"]["value"], 5);
    assert_eq!(body["hits"]["hits"], json!([]));

    let (_, body) = search(
        &app,
        "orders-2026",
        json!({"query": {"terms": {"order_id": ["ORD-109002", "ORD-109003", "nope"]}}}),
    )
    .await;
    assert_eq!(ids(&body), ["ORD-109002", "ORD-109003"]);

    let (_, body) = search(
        &app,
        "orders-2026",
        json!({"query": {"bool": {
            "must": [{"match": {"customer_name": "jordan"}}],
            "filter": [{"range": {"total": {"gte": 50}}}],
            "must_not": [{"term": {"status": "pending"}}],
            "should": [{"term": {"items": 3}}]
        }}, "sort": [{"created_at": "asc"}]}),
    )
    .await;
    assert_eq!(
        ids(&body),
        ["ORD-108815", "ORD-108816", "ORD-109002", "ORD-109003"]
    );
    assert_eq!(
        body["hits"]["hits"][0]["_score"],
        Value::Null,
        "sorted by field"
    );
    assert_eq!(
        body["hits"]["hits"][0]["sort"],
        json!([1_792_692_224_000_i64])
    );

    let (_, body) = search(
        &app,
        "orders-2026",
        json!({"query": {"bool": {"should": [{"term": {"status": "refunded"}}, {"term": {"status": "pending"}}, {"term": {"items": 4}}], "minimum_should_match": 2}}}),
    )
    .await;
    assert_eq!(body["hits"]["total"]["value"], 0);
}

#[tokio::test]
async fn ranges_with_date_math_prefix_wildcard_exists_ids() {
    let app = app();
    let (_, body) = search(
        &app,
        "logs-checkout-prod-*",
        json!({"query": {"bool": {"filter": [{"range": {"@timestamp": {"gte": "now-1h"}}}, {"range": {"status_code": {"gte": 500}}}]}}, "size": 100}),
    )
    .await;
    assert_eq!(body["hits"]["total"]["value"], 12);
    assert_eq!(body["hits"]["hits"][0]["_score"], 0.0);

    let (_, body) = search(
        &app,
        "logs-checkout-prod-*",
        json!({"query": {"range": {"@timestamp": {"gte": "now-1d/d", "lt": "now/d"}}}}),
    )
    .await;
    assert_eq!(body["hits"]["total"]["value"], 12, "all of yesterday");

    let (_, body) = search(
        &app,
        "orders-2026",
        json!({"query": {"prefix": {"customer_email": "jordan.lei"}}}),
    )
    .await;
    assert_eq!(body["hits"]["total"]["value"], 2);
    let (_, body) = search(&app, "orders-2026", json!({"query": {"wildcard": {"customer_email": {"value": "*LEE@*", "case_insensitive": true}}}})).await;
    assert_eq!(body["hits"]["total"]["value"], 6);
    let (_, body) = search(
        &app,
        "orders-2026",
        json!({"query": {"ids": {"values": ["ORD-109400"]}}}),
    )
    .await;
    assert_eq!(ids(&body), ["ORD-109400"]);
    let (_, body) = search(
        &app,
        "metrics-*",
        json!({"query": {"bool": {"must_not": {"exists": {"field": "cpu_pct"}}}}}),
    )
    .await;
    assert_eq!(body["hits"]["total"]["value"], 0);
}

#[tokio::test]
async fn query_string_and_url_q() {
    let app = app();
    let (_, body) = search(
        &app,
        "logs-checkout-prod-2026.10.30",
        json!({"query": {"query_string": {"query": "service:cart-api AND status_code:>=500"}}}),
    )
    .await;
    assert_eq!(body["hits"]["total"]["value"], 12);
    let (_, body) = search(&app, "logs-checkout-prod-2026.10.30", json!({"query": {"query_string": {"query": "\"upstream timeout\" OR deadlock", "default_field": "message"}}})).await;
    assert_eq!(body["hits"]["total"]["value"], 8);
    let (_, _, text) = send(
        &app,
        "GET",
        "/orders-2026/_search?q=status:refunded",
        None,
        true,
    )
    .await;
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(ids(&body), ["ORD-109311"]);
}

#[tokio::test]
async fn paging_source_filtering_and_total_hits() {
    let app = app();
    let (_, body) = search(&app, "orders-2026", json!({"sort": [{"total": "desc"}], "from": 1, "size": 2, "_source": ["order_id", "total"]})).await;
    assert_eq!(ids(&body), ["ORD-108815", "ORD-108816"]);
    assert_eq!(
        body["hits"]["hits"][0]["_source"],
        json!({"order_id": "ORD-108815", "total": 129.99})
    );
    let (_, body) = search(&app, "orders-2026", json!({"size": 1, "_source": {"excludes": ["customer_*", "payment_intent"]}, "sort": ["order_id"]})).await;
    assert_eq!(
        body["hits"]["hits"][0]["_source"],
        json!({"order_id": "ORD-104771", "created_at": "2026-08-14T09:12:00Z", "total": 42.0, "currency": "USD", "status": "fulfilled", "items": 1})
    );
    let (_, body) = search(
        &app,
        "orders-2026",
        json!({"size": 1, "_source": false, "track_total_hits": false}),
    )
    .await;
    assert!(body["hits"]["hits"][0].get("_source").is_none());
    assert!(body["hits"].get("total").is_none());
    let (_, body) = search(
        &app,
        "orders-2026",
        json!({"size": 1, "track_total_hits": 3}),
    )
    .await;
    assert_eq!(
        body["hits"]["total"],
        json!({"value": 3, "relation": "gte"})
    );
    let (_, body) = search(
        &app,
        "orders-2026",
        json!({"size": 1, "fields": ["order_id"], "_source": false}),
    )
    .await;
    assert_eq!(
        body["hits"]["hits"][0]["fields"],
        json!({"order_id": ["ORD-104771"]})
    );
}

#[tokio::test]
async fn aggregations() {
    let app = app();
    let (_, body) = search(
        &app,
        "logs-checkout-prod-2026.10.30",
        json!({"size": 0, "query": {"range": {"status_code": {"gte": 500}}}, "aggs": {
            "by_service": {"terms": {"field": "service", "size": 2}, "aggs": {"avg_ms": {"avg": {"field": "duration_ms"}}, "msgs": {"cardinality": {"field": "message.keyword"}}}},
            "total_ms": {"sum": {"field": "duration_ms"}},
            "slowest": {"max": {"field": "duration_ms"}},
            "per_hour": {"date_histogram": {"field": "@timestamp", "fixed_interval": "1h"}}
        }}),
    )
    .await;
    let aggs = &body["aggregations"];
    let buckets = aggs["by_service"]["buckets"].as_array().unwrap();
    assert_eq!(buckets[0]["key"], "cart-api");
    assert_eq!(buckets[0]["doc_count"], 12);
    assert_eq!(buckets[0]["msgs"]["value"], 2);
    assert_eq!(buckets[1]["key"], "payment-gateway");
    assert_eq!(buckets[1]["doc_count"], 7);
    assert_eq!(aggs["by_service"]["sum_other_doc_count"], 2);
    assert_eq!(aggs["slowest"]["value"], 30012.0);
    assert_eq!(
        aggs["per_hour"]["buckets"],
        json!([
            {"key_as_string": "2026-10-30T12:00:00.000Z", "key": 1_793_361_600_000_i64, "doc_count": 9},
            {"key_as_string": "2026-10-30T13:00:00.000Z", "key": 1_793_365_200_000_i64, "doc_count": 12}
        ])
    );
    assert!(body["hits"]["hits"].as_array().unwrap().is_empty());

    let (status, body) = search(
        &app,
        "logs-checkout-prod-2026.10.30",
        json!({"size": 0, "aggs": {"m": {"terms": {"field": "message"}}}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "search_phase_execution_exception");
    assert_eq!(
        body["error"]["root_cause"][0]["type"],
        "illegal_argument_exception"
    );
    assert!(
        body["error"]["root_cause"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("Fielddata is disabled on [message]")
    );

    let (_, body) = search(&app, "orders-2026", json!({"size": 0, "aggs": {"st": {"stats": {"field": "total"}}, "by_status": {"terms": {"field": "status", "order": {"_key": "asc"}}}}})).await;
    assert_eq!(body["aggregations"]["st"]["count"], 10);
    assert_eq!(
        body["aggregations"]["by_status"]["buckets"][0]["key"],
        "fulfilled"
    );
}

#[tokio::test]
async fn dsl_errors_look_like_elasticsearch() {
    let app = app();
    let (status, body) = search(
        &app,
        "orders-2026",
        json!({"query": {"mach": {"status": "paid"}}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "parsing_exception");
    assert_eq!(body["error"]["reason"], "unknown query [mach]");
    let (status, body) = search(&app, "orders-2026", json!({"qurey": {}})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["reason"],
        "Unknown key for a START_OBJECT in [qurey]."
    );
    let (status, body) = search(&app, "nope", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["type"], "index_not_found_exception");
    let (status, body) = search(&app, "orders-2025", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "index_closed_exception");
    let (status, body) = search(&app, "orders-2026", json!({"sort": [{"nope": "asc"}]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["root_cause"][0]["reason"],
        "No mapping found for [nope] in order to sort on"
    );
    let (status, _, text) = send(&app, "POST", "/orders-2026/_search", None, true).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let (status, body) = get(&app, "/orders-2026/_doc/1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"],
        "no handler found for uri [/orders-2026/_doc/1] and method [GET]"
    );
}

#[tokio::test]
async fn count_endpoint() {
    let app = app();
    let (_, _, text) = send(
        &app,
        "POST",
        "/orders-*/_count",
        Some(json!({"query": {"term": {"status": "paid"}}})),
        true,
    )
    .await;
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["count"], 7);
}

#[tokio::test]
async fn esql_stats_sort_limit_keep() {
    let app = app();
    let (status, body) = esql(
        &app,
        "FROM logs-checkout-prod-* | WHERE @timestamp >= NOW() - 1 hour AND status_code >= 500 | STATS errors = COUNT(*), avg_ms = AVG(duration_ms) BY service | SORT errors DESC | LIMIT 2",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["columns"],
        json!([{"name": "errors", "type": "long"}, {"name": "avg_ms", "type": "double"}, {"name": "service", "type": "keyword"}])
    );
    assert_eq!(body["values"][0][0], 7);
    assert_eq!(body["values"][0][2], "payment-gateway");
    assert_eq!(body["values"][1][2], "cart-api");
    assert_eq!(body["values"].as_array().unwrap().len(), 2);
    assert_eq!(body["is_partial"], false);

    let (_, body) = esql(&app, "FROM orders-2026 | WHERE customer_email LIKE \"jordan.lee*\" AND status IN (\"paid\", \"pending\") | EVAL cents = TO_INTEGER(total * 100) | SORT created_at DESC | KEEP order_id, cents | LIMIT 2").await;
    assert_eq!(
        body["columns"],
        json!([{"name": "order_id", "type": "keyword"}, {"name": "cents", "type": "integer"}])
    );
    assert_eq!(
        body["values"],
        json!([["ORD-109777", 6400], ["ORD-109003", 8640]])
    );

    let (_, body) = esql(&app, "FROM orders-2026 | STATS n = COUNT(*), revenue = ROUND(SUM(total), 2) BY bucket = DATE_TRUNC(1 day, created_at) | SORT bucket | LIMIT 1").await;
    assert_eq!(
        body["values"],
        json!([[1, 42.0, "2026-08-14T00:00:00.000Z"]])
    );

    let (_, body) = esql(
        &app,
        "FROM orders-2026 | STATS c = COUNT_DISTINCT(customer_email), MAX(total)",
    )
    .await;
    assert_eq!(body["columns"][1]["name"], "MAX(total)");
    assert_eq!(body["values"], json!([[5, 230.1]]));
}

#[tokio::test]
async fn esql_from_returns_sorted_columns_and_default_limit() {
    let app = app();
    let (_, body) = esql(&app, "FROM metrics-checkout-2026.10.30").await;
    let names: Vec<&str> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["@timestamp", "cpu_pct", "service"]);
    assert_eq!(body["values"].as_array().unwrap().len(), 6);
    assert_eq!(body["values"][0][0], "2026-10-30T13:00:00.000Z");
    let (_, body) = esql(
        &app,
        "FROM logs-checkout-prod-2026.10.30 | KEEP message* | LIMIT 1",
    )
    .await;
    assert_eq!(
        body["columns"],
        json!([{"name": "message", "type": "text"}, {"name": "message.keyword", "type": "keyword"}])
    );
    let (_, body) = esql(
        &app,
        "from orders-2026 metadata _id | where total is null | keep _id",
    )
    .await;
    assert_eq!(body["values"], json!([]));
}

#[tokio::test]
async fn esql_errors() {
    let app = app();
    let (status, body) = esql(&app, "FROM orders-2026 | WHERE nope > 1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "verification_exception");
    assert_eq!(
        body["error"]["reason"],
        "Found 1 problem\nline 1:26: Unknown column [nope]"
    );
    let (status, body) = esql(&app, "FROM nope | LIMIT 1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["reason"],
        "Found 1 problem\nline 1:1: Unknown index [nope]"
    );
    let (status, body) = esql(&app, "FROM orders-2026 | FILTER x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "parsing_exception");
    assert!(
        body["error"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("line 1:20: mismatched input 'FILTER'")
    );
    let (status, body) = esql(&app, "FROM orders-2026 | WHERE status = \"paid\"").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "parsing_exception");
    let (status, _, _) = send(&app, "POST", "/_query", Some(json!({})), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
