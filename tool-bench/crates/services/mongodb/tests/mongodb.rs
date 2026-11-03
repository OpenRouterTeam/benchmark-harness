use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_mongodb::Mongodb;
use serde_json::{Value, json};
use tower::ServiceExt;

fn world(scenario: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/cloud/mongodb")
        .join(scenario);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

async fn call(app: &Router, name: &str, arguments: Value) -> Value {
    let req = Request::post("/_mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "name": name, "arguments": arguments }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn first_text(r: &Value) -> &str {
    r["content"][0]["text"].as_str().unwrap()
}

fn all_text(r: &Value) -> String {
    r["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["mongodb"].clone()
}

const PRE: &str = "preconfigured";

#[tokio::test]
async fn find_filters_sorts_projects_and_wraps_untrusted_data() {
    let app = service_app(world("refund-order"), &Mongodb);
    let r = call(
        &app,
        "find",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders",
               "filter": {"customer.email": "priya.natarajan@example.com"},
               "projection": {"_id": 0, "orderNumber": 1, "total": 1}, "sort": {"orderNumber": -1}, "limit": 2}),
    )
    .await;
    assert!(r.get("isError").is_none(), "{r}");
    assert_eq!(
        first_text(&r),
        "Query on collection \"orders\" resulted in 3 documents. Returning 2 documents."
    );
    assert_eq!(
        r["structuredContent"]["documents"],
        json!([{"orderNumber": 10517, "total": 649}, {"orderNumber": 10482, "total": 214.5}])
    );
    let wrapped = r["content"][1]["text"].as_str().unwrap();
    assert!(wrapped.contains("<untrusted-user-data-"));
    assert!(
        wrapped
            .contains(r#"[{"orderNumber":10517,"total":649},{"orderNumber":10482,"total":214.5}]"#)
    );

    // EJSON dates and ObjectIds in filters.
    let r = call(
        &app,
        "find",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders",
               "filter": {"_id": {"$oid": "65f1a000000000000000000a"}, "createdAt": {"$gte": {"$date": "2026-02-01T00:00:00Z"}}}}),
    )
    .await;
    assert_eq!(r["structuredContent"]["documents"][0]["orderNumber"], 10482);
    assert_eq!(
        r["structuredContent"]["documents"][0]["createdAt"],
        json!({"$date": "2026-02-16T15:27:00.000Z"})
    );

    // The configured 100-document cap is reported.
    let r = call(
        &app,
        "find",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "limit": 500}),
    )
    .await;
    assert_eq!(
        r["structuredContent"]["appliedLimits"],
        json!(["config.maxDocumentsPerQuery"])
    );
    assert_eq!(r["structuredContent"]["queryResultsCount"], 17);
}

#[tokio::test]
async fn aggregate_groups_revenue_and_counts() {
    let app = service_app(world("top-category-revenue"), &Mongodb);
    let r = call(
        &app,
        "aggregate",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "pipeline": [
            {"$match": {"status": "delivered", "createdAt": {"$gte": {"$date": "2026-02-01T00:00:00.000Z"}, "$lt": {"$date": "2026-03-01T00:00:00.000Z"}}}},
            {"$unwind": "$items"},
            {"$group": {"_id": "$items.category", "revenue": {"$sum": {"$multiply": ["$items.price", "$items.qty"]}}}},
            {"$sort": {"revenue": -1}}
        ]}),
    )
    .await;
    assert_eq!(first_text(&r), "The aggregation resulted in 4 documents.");
    assert_eq!(
        r["structuredContent"]["documents"],
        json!([
            {"_id": "outdoor", "revenue": 1457.5},
            {"_id": "electronics", "revenue": 1309},
            {"_id": "kitchen", "revenue": 798},
            {"_id": "apparel", "revenue": 436}
        ])
    );
    assert_eq!(r["structuredContent"]["count"], 4);

    let r = call(
        &app,
        "aggregate",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "pipeline": [
            {"$lookup": {"from": "customers", "localField": "customer.customerId", "foreignField": "_id", "as": "c"}},
            {"$match": {"c.tier": "gold"}},
            {"$count": "n"}
        ]}),
    )
    .await;
    assert_eq!(r["structuredContent"]["documents"], json!([{"n": 6}]));

    let r = call(
        &app,
        "count",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "query": {"status": {"$in": ["cancelled", "refunded"]}}}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Found 2 documents in the collection \"orders\" that matched the query."
    );

    let r = call(
        &app,
        "aggregate",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "pipeline": [{"$bogus": {}}]}),
    )
    .await;
    assert_eq!(r["isError"], true);
    assert_eq!(
        first_text(&r),
        "Error running aggregate: Unrecognized pipeline stage name: '$bogus'"
    );

    let r = call(
        &app,
        "aggregate-db",
        json!({"connectionId": PRE, "database": "shop", "pipeline": [{"$match": {}}]}),
    )
    .await;
    assert!(first_text(&r).contains("must be a database-level aggregation stage"));
}

#[tokio::test]
async fn update_many_changes_state_in_place() {
    let w = world("refund-order");
    let app = service_app(w.clone(), &Mongodb);
    let r = call(
        &app,
        "update-many",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "filter": {"orderNumber": 10482},
               "update": {"$set": {"status": "refunded", "refund.reason": "damaged in transit", "refund.amount": 214.5}}}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Matched 1 document(s). Modified 1 document(s)."
    );
    let s = state(&w);
    let doc = &s["db.shop.orders"]["65f1a000000000000000000a"];
    assert_eq!(doc["status"], "refunded");
    assert_eq!(
        doc["refund"],
        json!({"reason": "damaged in transit", "amount": 214.5})
    );
    let keys: Vec<&String> = s["db.shop.orders"].as_object().unwrap().keys().collect();
    assert_eq!(
        keys[9], "65f1a000000000000000000a",
        "updated document keeps its position"
    );

    let r = call(
        &app,
        "update-many",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "filter": {"orderNumber": 1}, "update": {"$set": {"x": 1}}}),
    )
    .await;
    assert_eq!(first_text(&r), "No documents matched the filter.");

    let r = call(
        &app,
        "update-many",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "filter": {}, "update": {"status": "x"}}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Error running update-many: Update document requires atomic operators"
    );

    // Unique index on orderNumber.
    let r = call(
        &app,
        "update-many",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "filter": {"orderNumber": 10428}, "update": {"$set": {"orderNumber": 10482}}}),
    )
    .await;
    assert!(first_text(&r).contains("E11000 duplicate key error collection: shop.orders index: orderNumber_1 dup key: { orderNumber: 10482 }"), "{r}");
}

#[tokio::test]
async fn insert_and_delete_many() {
    let w = world("purge-qa-accounts");
    let app = service_app(w.clone(), &Mongodb);
    let r = call(
        &app,
        "delete-many",
        json!({"connectionId": PRE, "database": "crm", "collection": "customers", "filter": {"email": {"$regex": "@qa\\.octoshop\\.test$"}}}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Deleted `4` document(s) from the requested collection."
    );
    let s = state(&w);
    assert!(
        s["db.crm.customers"]
            .get("65e3c0000000000000000002")
            .is_none()
    );
    assert!(
        s["db.crm.customers"]
            .get("65e3c0000000000000000006")
            .is_some()
    );
    assert_eq!(s["db.crm_staging.customers"].as_object().unwrap().len(), 5);

    let r = call(
        &app,
        "insert-many",
        json!({"connectionId": PRE, "database": "crm", "collection": "customers", "documents": [{"name": "New", "email": "new@example.com"}]}),
    )
    .await;
    assert_eq!(first_text(&r), "Documents were inserted successfully.");
    let id = r["structuredContent"]["insertedIds"][0]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(id.len(), 24);
    assert_eq!(
        state(&w)["db.crm.customers"][&id]["email"],
        "new@example.com"
    );

    let r = call(
        &app,
        "insert-many",
        json!({"connectionId": PRE, "database": "crm", "collection": "customers", "documents": [{"email": "raj.patel@example.in"}]}),
    )
    .await;
    assert_eq!(r["isError"], true);
    assert!(
        first_text(&r)
            .contains("E11000 duplicate key error collection: crm.customers index: email_1")
    );
}

#[tokio::test]
async fn metadata_tools() {
    let w = world("refund-order");
    let app = service_app(w.clone(), &Mongodb);
    let r = call(&app, "list-databases", json!({"connectionId": PRE})).await;
    let names: Vec<&str> = r["structuredContent"]["databases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["admin", "local", "shop", "shop_staging"]);

    let r = call(
        &app,
        "list-collections",
        json!({"connectionId": PRE, "database": "shop"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Found 3 collections in the requested database."
    );
    let r = call(
        &app,
        "list-collections",
        json!({"connectionId": PRE, "database": "nope"}),
    )
    .await;
    assert!(first_text(&r).starts_with("Found 0 collections"));

    let r = call(
        &app,
        "collection-indexes",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Found 4 classic indexes in the requested collection:"
    );
    let r = call(
        &app,
        "collection-indexes",
        json!({"connectionId": PRE, "database": "shop", "collection": "nope"}),
    )
    .await;
    assert_eq!(r["isError"], true);

    let r = call(&app, "create-index", json!({"connectionId": PRE, "database": "shop", "collection": "orders", "definition": [{"type": "classic", "keys": {"total": -1}}]})).await;
    assert_eq!(
        first_text(&r),
        "Created the index \"total_-1\" on collection \"orders\" in database \"shop\"."
    );
    let r = call(&app, "drop-index", json!({"connectionId": PRE, "database": "shop", "collection": "orders", "indexName": "total_-1", "type": "classic"})).await;
    assert_eq!(
        first_text(&r),
        "Successfully dropped the index from the provided namespace."
    );
    let r = call(&app, "drop-index", json!({"connectionId": PRE, "database": "shop", "collection": "orders", "indexName": "total_-1", "type": "classic"})).await;
    assert_eq!(
        first_text(&r),
        "Error running drop-index: index not found with name [total_-1]"
    );

    let r = call(
        &app,
        "collection-schema",
        json!({"connectionId": PRE, "database": "shop", "collection": "customers"}),
    )
    .await;
    assert_eq!(
        r["structuredContent"]["schema"]["_id"],
        json!({"types": [{"bsonType": "ObjectId"}]})
    );
    assert_eq!(r["structuredContent"]["fieldsCount"], 5);

    let r = call(
        &app,
        "collection-storage-size",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders"}),
    )
    .await;
    assert!(first_text(&r).starts_with("The size of the requested namespace is `"));
    let r = call(
        &app,
        "db-stats",
        json!({"connectionId": PRE, "database": "shop"}),
    )
    .await;
    assert_eq!(r["structuredContent"]["stats"]["collections"], 3);
    assert_eq!(r["structuredContent"]["stats"]["objects"], 6 + 17 + 28);

    let r = call(&app, "explain", json!({"connectionId": PRE, "database": "shop", "collection": "orders", "method": [{"name": "find", "arguments": {"filter": {"orderNumber": 10482}}}], "verbosity": "executionStats"})).await;
    let plan = &r["structuredContent"]["explainResult"];
    assert_eq!(
        plan["queryPlanner"]["winningPlan"]["inputStage"]["indexName"],
        "orderNumber_1"
    );
    assert_eq!(plan["executionStats"]["nReturned"], 1);

    let r = call(
        &app,
        "mongodb-logs",
        json!({"connectionId": PRE, "limit": 2}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Found: 5 messages (showing only the first 2)"
    );

    let r = call(&app, "export", json!({"connectionId": PRE, "database": "shop", "collection": "orders", "exportTitle": "Feb orders", "exportTarget": [{"name": "find", "arguments": {"filter": {}}}]})).await;
    assert_eq!(r["content"][1]["type"], "resource_link");

    let r = call(&app, "rename-collection", json!({"connectionId": PRE, "database": "shop", "collection": "products", "newName": "orders"})).await;
    assert_eq!(r["isError"], true);
    let r = call(&app, "rename-collection", json!({"connectionId": PRE, "database": "shop", "collection": "products", "newName": "catalog"})).await;
    assert_eq!(r["structuredContent"]["renamed"], true);
    assert_eq!(state(&w)["db.shop.catalog"].as_object().unwrap().len(), 28);

    let r = call(
        &app,
        "create-collection",
        json!({"connectionId": PRE, "database": "shop", "collection": "returns"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Collection \"returns\" created in database \"shop\"."
    );
    let r = call(
        &app,
        "drop-collection",
        json!({"connectionId": PRE, "database": "shop", "collection": "returns"}),
    )
    .await;
    assert_eq!(r["structuredContent"]["dropped"], true);
    let r = call(
        &app,
        "drop-database",
        json!({"connectionId": PRE, "database": "shop_staging"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Successfully dropped the requested database."
    );
    assert!(
        state(&w)["db.shop_staging.orders"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn connections_and_validation_errors() {
    let app = service_app(world("refund-order"), &Mongodb);
    let r = call(&app, "list-connections", json!({})).await;
    assert!(first_text(&r).contains("\"preconfigured\" (connectionId: \"preconfigured\", disconnected): Configured connection string (not yet dialed)"));

    let r = call(
        &app,
        "find",
        json!({"connectionId": "nope", "database": "shop", "collection": "orders"}),
    )
    .await;
    assert_eq!(r["isError"], true);
    assert!(first_text(&r).starts_with("Connection \"nope\" does not exist or has expired."));
    assert!(
        r["content"][1]["text"]
            .as_str()
            .unwrap()
            .contains("\"atlas-connect-cluster\", \"connect\"")
    );

    let r = call(
        &app,
        "find",
        json!({"database": "shop", "collection": "orders"}),
    )
    .await;
    assert!(
        first_text(&r).starts_with(
            "MCP error -32602: Input validation error: Invalid arguments for tool find"
        )
    );
    let r = call(
        &app,
        "find",
        json!({"connectionId": PRE, "database": "shop", "collection": "orders", "bogus": 1}),
    )
    .await;
    assert!(
        first_text(&r).contains("Unrecognized key: \\\"bogus\\\""),
        "{r}"
    );

    let r = call(&app, "connect", json!({"connectionString": "postgres://x"})).await;
    assert!(first_text(&r).contains("Invalid scheme"));
    let r = call(
        &app,
        "connect",
        json!({"connectionString": "mongodb+srv://elsewhere.example.com"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Error running connect: querySrv ENOTFOUND _mongodb._tcp.elsewhere.example.com"
    );
    let r = call(&app, "connect", json!({"connectionString": "mongodb://octo-prod-shard-00-00.x7kq2.mongodb.net:27017/", "connectionName": "Prod Primary"})).await;
    let id = r["structuredContent"]["connectionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = call(
        &app,
        "count",
        json!({"connectionId": id, "database": "shop", "collection": "orders"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Found 17 documents in the collection \"orders\"."
    );
    let r = call(&app, "list-connections", json!({})).await;
    assert!(first_text(&r).contains("\"prod-primary-"));
    let r = call(&app, "disconnect", json!({"connectionId": id})).await;
    assert_eq!(r["structuredContent"]["outcome"], "removed");
}

#[tokio::test]
async fn atlas_projects_clusters_and_pause() {
    let w = world("pause-idle-staging-cluster");
    let app = service_app(w.clone(), &Mongodb);
    let r = call(&app, "atlas-list-orgs", json!({})).await;
    assert_eq!(r["structuredContent"]["totalCount"], 2);
    let r = call(
        &app,
        "atlas-list-projects",
        json!({"orgId": "65a1b2c3d4e5f60718293a4b"}),
    )
    .await;
    assert_eq!(first_text(&r), "Found 3 projects.");
    assert_eq!(
        r["structuredContent"]["projects"][0]["created"],
        "3/4/2024, 10:12:00 AM"
    );

    let r = call(
        &app,
        "atlas-list-clusters",
        json!({"projectId": "65c0ffee0000000000000002"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Found 2 clusters in project 65c0ffee0000000000000002:"
    );
    let c = &r["structuredContent"]["clusters"][1];
    assert_eq!(c["name"], "analytics-staging");
    assert_eq!(c["instanceType"], "DEDICATED");
    assert_eq!(c["instanceSize"], "M10");
    assert_eq!(
        c["processIds"][0],
        "analytics-staging-shard-00-00.p3n8d.mongodb.net:27017"
    );

    let r = call(&app, "atlas-list-clusters", json!({})).await;
    assert_eq!(first_text(&r), "Found 6 clusters across all projects");
    let r = call(
        &app,
        "atlas-list-clusters",
        json!({"projectId": "65c0ffee00000000000000ff"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Error running atlas-list-clusters: [404 Not Found] error calling Atlas API: Not Found; No group with ID 65c0ffee00000000000000ff exists."
    );

    let r = call(
        &app,
        "atlas-inspect-cluster",
        json!({"projectId": "65c0ffee0000000000000004", "clusterName": "ds-sandbox"}),
    )
    .await;
    assert_eq!(r["structuredContent"]["instanceType"], "FREE");
    assert_eq!(r["structuredContent"]["instanceSize"], "N/A");

    let r = call(&app, "atlas-pause-resume-cluster", json!({"projectId": "65c0ffee0000000000000002", "clusterName": "analytics-staging", "action": "PAUSE"})).await;
    assert!(first_text(&r).starts_with(
        "Cluster \"analytics-staging\" in project \"65c0ffee0000000000000002\" is being paused."
    ));
    let s = state(&w);
    assert_eq!(
        s["clusters"]["65c0ffee0000000000000002/analytics-staging"]["paused"],
        true
    );
    assert_eq!(
        s["clusters"]["65c0ffee0000000000000003/analytics-staging"]["paused"],
        false
    );
    let r = call(&app, "atlas-pause-resume-cluster", json!({"projectId": "65c0ffee0000000000000002", "clusterName": "analytics-staging", "action": "PAUSE"})).await;
    assert_eq!(r["isError"], true);
    let r = call(&app, "atlas-pause-resume-cluster", json!({"projectId": "65c0ffee0000000000000004", "clusterName": "ds-sandbox", "action": "PAUSE"})).await;
    assert!(first_text(&r).contains("[400 Bad Request]"));
}

#[tokio::test]
async fn atlas_users_access_alerts_advisor_and_connect() {
    let w = world("pause-idle-staging-cluster");
    let app = service_app(w.clone(), &Mongodb);
    let pid = "65c0ffee0000000000000002";
    let r = call(&app, "atlas-list-db-users", json!({"projectId": pid})).await;
    assert_eq!(
        r["structuredContent"]["users"][0]["username"],
        "analytics-etl"
    );
    let r = call(&app, "atlas-create-db-user", json!({"projectId": pid, "username": "report-ro", "roles": [{"roleName": "read", "databaseName": "analytics"}]})).await;
    assert!(first_text(&r).starts_with("User \"report-ro\" created successfully with password: `"));
    assert_eq!(all_text(&r).matches("IP access list").count(), 1);
    assert!(state(&w)["db_users"]["65c0ffee0000000000000002/report-ro"].is_object());

    let r = call(&app, "atlas-inspect-access-list", json!({"projectId": pid})).await;
    assert_eq!(first_text(&r), "Found 2 access list entries");
    let r = call(
        &app,
        "atlas-create-access-list",
        json!({"projectId": pid, "cidrBlocks": ["10.30.0.0/16"]}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        format!("IP/CIDR ranges added to access list for project {pid}.")
    );

    let r = call(&app, "atlas-list-alerts", json!({"projectId": pid})).await;
    assert_eq!(
        first_text(&r),
        format!("Found 1 alerts with status \"OPEN\" in project {pid}")
    );
    assert_eq!(
        r["structuredContent"]["alerts"][0]["created"],
        "2026-03-10T02:00:00.000Z"
    );
    let r = call(
        &app,
        "atlas-list-alerts",
        json!({"projectId": pid, "status": "CLOSED"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "No alerts with status \"CLOSED\" found in your MongoDB Atlas project."
    );

    let r = call(
        &app,
        "atlas-get-performance-advisor",
        json!({"projectId": "65c0ffee0000000000000001", "clusterName": "shop-prod"}),
    )
    .await;
    assert_eq!(first_text(&r), "Performance advisor data");
    assert!(
        r["content"][1]["text"]
            .as_str()
            .unwrap()
            .contains("## Suggested Indexes")
    );
    let r = call(
        &app,
        "atlas-get-performance-advisor",
        json!({"projectId": pid, "clusterName": "shop-staging"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "No performance advisor recommendations found."
    );

    let r = call(
        &app,
        "atlas-connect-cluster",
        json!({"projectId": "65c0ffee0000000000000001", "clusterName": "shop-prod"}),
    )
    .await;
    assert_eq!(r["structuredContent"]["state"], "connected");
    assert_eq!(r["structuredContent"]["createdTemporaryUser"], true);
    let id = r["structuredContent"]["connectionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = call(&app, "list-connections", json!({})).await;
    assert!(
        first_text(&r).contains("Atlas cluster \"shop-prod\" (project 65c0ffee0000000000000001)")
    );
    let r = call(&app, "list-databases", json!({"connectionId": id})).await;
    assert!(r.get("isError").is_none());

    let r = call(&app, "atlas-get-regions", json!({"provider": "AWS"})).await;
    assert_eq!(r["structuredContent"]["regions"][0]["name"], "US_EAST_1");
    let r = call(
        &app,
        "atlas-create-free-cluster",
        json!({"projectId": pid, "name": "scratch"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Cluster \"scratch\" has been created in region \"US_EAST_1\"."
    );
    let r = call(
        &app,
        "atlas-inspect-cluster",
        json!({"projectId": pid, "clusterName": "scratch"}),
    )
    .await;
    assert_eq!(r["structuredContent"]["state"], "IDLE");
    let r = call(
        &app,
        "atlas-upgrade-cluster",
        json!({"projectId": pid, "clusterName": "shop-staging", "targetTier": "M20"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Cluster \"shop-staging\" is being scaled to M20. This may take a few minutes."
    );
    let r = call(
        &app,
        "atlas-create-project",
        json!({"projectName": "octo-search", "orgId": "65a1b2c3d4e5f60718293a4b"}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Project \"octo-search\" created successfully."
    );
}

#[tokio::test]
async fn streams_and_assistant() {
    let app = service_app(world("pause-idle-staging-cluster"), &Mongodb);
    let pid = "65c0ffee0000000000000002";
    let r = call(
        &app,
        "atlas-streams-discover",
        json!({"projectId": pid, "action": "list-workspaces"}),
    )
    .await;
    assert!(first_text(&r).starts_with("No Stream Processing workspaces found"));
    let r = call(&app, "atlas-streams-build", json!({"projectId": pid, "resource": "workspace", "workspaceName": "events", "cloudProvider": "AWS", "region": "VIRGINIA_USA"})).await;
    assert!(first_text(&r).starts_with("Workspace 'events' created in AWS/VIRGINIA_USA (SP10)."));
    let r = call(&app, "atlas-streams-build", json!({"projectId": pid, "resource": "processor", "workspaceName": "events", "processorName": "p1", "pipeline": [{"$source": {"connectionName": "sample_stream_solar"}}]})).await;
    assert!(first_text(&r).starts_with("Processor created in CREATED state."));
    let r = call(&app, "atlas-streams-manage", json!({"projectId": pid, "workspaceName": "events", "action": "start-processor", "resourceName": "p1"})).await;
    assert_eq!(r["structuredContent"]["processorState"], "STARTED");
    let r = call(
        &app,
        "atlas-streams-teardown",
        json!({"projectId": pid, "resource": "workspace", "workspaceName": "events"}),
    )
    .await;
    assert_eq!(r["isError"], true);

    let r = call(&app, "list-knowledge-sources", json!({})).await;
    assert_eq!(
        first_text(&r),
        "Found 4 data sources in the MongoDB Assistant knowledge base."
    );
    let r = call(
        &app,
        "search-knowledge",
        json!({"query": "how do I pause a cluster", "limit": 1}),
    )
    .await;
    assert_eq!(
        first_text(&r),
        "Found 1 results in the MongoDB Assistant knowledge base."
    );
    assert!(
        r["content"][1]["text"]
            .as_str()
            .unwrap()
            .contains("pause-terminate-cluster")
    );
}

#[tokio::test]
async fn unknown_routes_are_404() {
    let app = service_app(world("refund-order"), &Mongodb);
    let resp = app
        .oneshot(Request::get("/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
