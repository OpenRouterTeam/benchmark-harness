//! Exercises every GraphQL document `shopify-mcp` 1.0.8 sends (copied verbatim into
//! `tests/documents/`) against the fixture dataset.

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
use mock_shopify::Shopify;
use serde_json::{Value, json};
use tower::ServiceExt;

const URI: &str = "/admin/api/2026-01/graphql.json";

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn doc(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/documents")
        .join(format!("{name}.graphql"));
    std::fs::read_to_string(path).unwrap()
}

async fn post(app: &Router, uri: &str, token: Option<&str>, body: String) -> (StatusCode, Value) {
    let mut req = Request::post(uri)
        .header("host", "octo-corp.myshopify.com")
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("X-Shopify-Access-Token", t);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn gql_raw(app: &Router, query: &str, variables: Value) -> Value {
    let body = json!({"query": query, "variables": variables}).to_string();
    let (status, v) = post(app, URI, Some("shpat_toolbench"), body).await;
    assert_eq!(status, StatusCode::OK);
    v
}

async fn gql(app: &Router, name: &str, variables: Value) -> Value {
    gql_raw(app, &doc(name), variables).await
}

fn state(world: &SharedWorld) -> Value {
    world.lock().unwrap().snapshot()["services"]["shopify"].clone()
}

fn setup() -> (SharedWorld, Router) {
    let w = world();
    let app = service_app(w.clone(), &Shopify);
    (w, app)
}

#[tokio::test]
async fn rejects_missing_token_and_bad_bodies() {
    let (_, app) = setup();
    let (status, body) = post(&app, URI, None, "{}".into()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        body["errors"]
            .as_str()
            .unwrap()
            .contains("Invalid API key or access token")
    );
    let (status, body) = post(&app, URI, Some("shpat_toolbench"), "nope".into()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["errors"]["query"],
        "Required parameter missing or invalid"
    );
    let (status, _) = post(
        &app,
        "/admin/api/2026-01/products.json",
        Some("shpat_toolbench"),
        "{}".into(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_products_lists_and_searches_by_title() {
    let (_, app) = setup();
    let all = gql(&app, "GetProducts", json!({"first": 10})).await;
    let edges = all["data"]["products"]["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 3);
    let tote = &edges[0]["node"];
    assert_eq!(tote["id"], "gid://shopify/Product/8101");
    assert_eq!(tote["description"], "Heavy waxed canvas & leather handles.");
    assert_eq!(tote["totalInventory"], 17);
    assert_eq!(
        tote["priceRangeV2"]["minVariantPrice"],
        json!({"amount": "38.0", "currencyCode": "USD"})
    );
    assert_eq!(tote["images"]["edges"][0]["node"]["altText"], "Navy tote");
    assert_eq!(
        tote["variants"]["edges"][0]["node"],
        json!({"id": "gid://shopify/ProductVariant/4101", "title": "Navy", "price": "38.00", "inventoryQuantity": 12, "sku": "HCT-NVY"})
    );
    assert_eq!(
        edges[1]["node"]["variants"]["edges"][0]["node"]["title"],
        "Default Title"
    );
    assert!(
        all["extensions"]["cost"]["actualQueryCost"]
            .as_i64()
            .unwrap()
            > 0
    );

    let found = gql(
        &app,
        "GetProducts",
        json!({"first": 10, "query": "title:*canvas tote*"}),
    )
    .await;
    let titles: Vec<&str> = found["data"]["products"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["node"]["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Harbor Canvas Tote"]);
    let none = gql(
        &app,
        "GetProducts",
        json!({"first": 10, "query": "title:*kayak*"}),
    )
    .await;
    assert_eq!(none["data"]["products"]["edges"], json!([]));
}

#[tokio::test]
async fn connections_paginate_with_cursors() {
    let (_, app) = setup();
    let q = "query P($after: String) { products(first: 2, after: $after, sortKey: TITLE) { edges { cursor node { title } } pageInfo { hasNextPage hasPreviousPage endCursor } } }";
    let page1 = gql_raw(&app, q, json!({})).await;
    let p = &page1["data"]["products"];
    assert_eq!(p["edges"][0]["node"]["title"], "Alpine Wool Beanie");
    assert_eq!(p["pageInfo"]["hasNextPage"], true);
    assert_eq!(p["pageInfo"]["hasPreviousPage"], false);
    let page2 = gql_raw(&app, q, json!({"after": p["pageInfo"]["endCursor"]})).await;
    let p2 = &page2["data"]["products"];
    assert_eq!(p2["edges"].as_array().unwrap().len(), 1);
    assert_eq!(p2["edges"][0]["node"]["title"], "Linen Shirt");
    assert_eq!(
        p2["pageInfo"],
        json!({"hasNextPage": false, "hasPreviousPage": true, "endCursor": p2["edges"][0]["cursor"]})
    );

    let missing = gql_raw(&app, "{ products { edges { node { id } } } }", json!({})).await;
    assert_eq!(
        missing["errors"][0]["message"],
        "you must provide one of first or last"
    );
    let reversed = gql_raw(
        &app,
        "{ orders(first: 1, reverse: true) { nodes { name } } }",
        json!({}),
    )
    .await;
    assert_eq!(reversed["data"]["orders"]["nodes"][0]["name"], "#1003");
}

#[tokio::test]
async fn product_by_id_found_missing_and_invalid() {
    let (_, app) = setup();
    let found = gql(
        &app,
        "GetProductById",
        json!({"id": "gid://shopify/Product/8101"}),
    )
    .await;
    let p = &found["data"]["product"];
    assert_eq!(
        p["collections"]["edges"][0]["node"],
        json!({"id": "gid://shopify/Collection/301", "title": "Summer Essentials"})
    );
    assert_eq!(
        p["variants"]["edges"][1]["node"]["selectedOptions"],
        json!([{"name": "Color", "value": "Sand"}])
    );
    assert_eq!(p["images"]["edges"][0]["node"]["width"], 1200);
    assert_eq!(p["tags"], json!(["bags", "summer"]));
    assert_eq!(p["vendor"], "Octo Goods");

    let missing = gql(
        &app,
        "GetProductById",
        json!({"id": "gid://shopify/Product/9999"}),
    )
    .await;
    assert_eq!(missing["data"]["product"], Value::Null);
    let wrong_type = gql(
        &app,
        "GetProductById",
        json!({"id": "gid://shopify/Order/5501"}),
    )
    .await;
    assert_eq!(wrong_type["data"]["product"], Value::Null);
    let invalid = gql(&app, "GetProductById", json!({"id": "8101"})).await;
    assert_eq!(
        invalid["errors"][0]["message"],
        "Variable $id of type ID! was provided invalid value"
    );
    assert_eq!(
        invalid["errors"][0]["extensions"]["problems"][0]["explanation"],
        "Invalid global id '8101'"
    );
    assert!(invalid.get("data").is_none());
}

#[tokio::test]
async fn get_customers_search_and_spend() {
    let (_, app) = setup();
    let all = gql(&app, "GetCustomers", json!({"first": 10})).await;
    let customers = all["data"]["customers"]["edges"].as_array().unwrap();
    assert_eq!(customers.len(), 3);
    let jordan = &customers[0]["node"];
    // #1001 counts (2 x 38.00 + 8.00 + 6.08); the cancelled #1003 does not.
    assert_eq!(
        jordan["amountSpent"],
        json!({"amount": "90.08", "currencyCode": "USD"})
    );
    assert_eq!(jordan["numberOfOrders"], "2");
    assert_eq!(jordan["defaultAddress"]["provinceCode"], "OR");
    assert_eq!(jordan["addresses"].as_array().unwrap().len(), 1);
    assert_eq!(customers[2]["node"]["defaultAddress"], Value::Null);

    let sams = gql(
        &app,
        "GetCustomers",
        json!({"first": 10, "query": "Sam Ortiz"}),
    )
    .await;
    assert_eq!(
        sams["data"]["customers"]["edges"].as_array().unwrap().len(),
        2
    );
    let by_email = gql(
        &app,
        "GetCustomers",
        json!({"first": 10, "query": "email:sortiz@gmail.com"}),
    )
    .await;
    let e = by_email["data"]["customers"]["edges"].as_array().unwrap();
    assert_eq!(e.len(), 1);
    assert_eq!(e[0]["node"]["id"], "gid://shopify/Customer/7003");
    let tagged = gql(
        &app,
        "GetCustomers",
        json!({"first": 10, "query": "tag:wholesale"}),
    )
    .await;
    assert_eq!(
        tagged["data"]["customers"]["edges"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn get_orders_filters_by_status() {
    let (_, app) = setup();
    let names = |v: &Value| -> Vec<String> {
        v["data"]["orders"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["node"]["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let any = gql(&app, "GetOrders", json!({"first": 10})).await;
    assert_eq!(names(&any), ["#1001", "#1002", "#1003"]);
    let first = &any["data"]["orders"]["edges"][0]["node"];
    assert_eq!(
        first["totalPriceSet"]["shopMoney"],
        json!({"amount": "90.08", "currencyCode": "USD"})
    );
    assert_eq!(first["subtotalPriceSet"]["shopMoney"]["amount"], "76.0");
    assert_eq!(first["displayFulfillmentStatus"], "FULFILLED");
    assert_eq!(first["customer"]["email"], "jordan.lee@example.com");
    assert_eq!(
        first["lineItems"]["edges"][0]["node"]["originalTotalSet"]["shopMoney"]["amount"],
        "76.0"
    );
    assert_eq!(
        first["lineItems"]["edges"][0]["node"]["variant"],
        json!({"id": "gid://shopify/ProductVariant/4101", "title": "Navy", "sku": "HCT-NVY"})
    );
    assert_eq!(
        any["data"]["orders"]["edges"][2]["node"]["lineItems"]["edges"][0]["node"]["variant"],
        Value::Null
    );

    assert_eq!(
        names(
            &gql(
                &app,
                "GetOrders",
                json!({"first": 10, "query": "status:open"})
            )
            .await
        ),
        ["#1002"]
    );
    assert_eq!(
        names(
            &gql(
                &app,
                "GetOrders",
                json!({"first": 10, "query": "status:closed"})
            )
            .await
        ),
        ["#1001"]
    );
    assert_eq!(
        names(
            &gql(
                &app,
                "GetOrders",
                json!({"first": 10, "query": "status:cancelled"})
            )
            .await
        ),
        ["#1003"]
    );
    assert_eq!(
        names(&gql(&app, "GetOrders", json!({"first": 1})).await),
        ["#1001"]
    );
    assert_eq!(
        names(
            &gql(
                &app,
                "GetOrders",
                json!({"first": 10, "query": "financial_status:paid fulfillment_status:unshipped"})
            )
            .await
        ),
        ["#1002"]
    );
}

#[tokio::test]
async fn get_order_by_id_and_customer_orders() {
    let (_, app) = setup();
    let o = gql(
        &app,
        "GetOrderById",
        json!({"id": "gid://shopify/Order/5501"}),
    )
    .await;
    let order = &o["data"]["order"];
    assert_eq!(order["name"], "#1001");
    assert_eq!(order["customer"]["phone"], "+14155550101");
    assert_eq!(
        order["metafields"]["edges"][0]["node"],
        json!({"id": "gid://shopify/Metafield/1002", "namespace": "custom", "key": "gift_message", "value": "Happy birthday!", "type": "single_line_text_field"})
    );
    assert_eq!(order["shippingAddress"]["city"], "Portland");
    let missing = gql(&app, "GetOrderById", json!({"id": "gid://shopify/Order/1"})).await;
    assert_eq!(missing["data"]["order"], Value::Null);

    let co = gql(
        &app,
        "GetCustomerOrders",
        json!({"query": "customer_id:7001", "first": 10}),
    )
    .await;
    let names: Vec<&str> = co["data"]["orders"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["node"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["#1001", "#1003"]);
    let none = gql(
        &app,
        "GetCustomerOrders",
        json!({"query": "customer_id:42", "first": 10}),
    )
    .await;
    assert_eq!(none["data"]["orders"]["edges"], json!([]));
}

#[tokio::test]
async fn order_update_writes_state() {
    let (w, app) = setup();
    let r = gql(&app, "orderUpdate", json!({"input": {
        "id": "gid://shopify/Order/5502",
        "tags": ["VIP, priority"],
        "note": "Leave with concierge",
        "customAttributes": [{"key": "door", "value": "B"}],
        "metafields": [{"namespace": "custom", "key": "delivery", "value": "concierge", "type": "single_line_text_field"}],
        "shippingAddress": {"address1": "1 Main St", "city": "Austin", "province": "TX"}
    }})).await;
    let u = &r["data"]["orderUpdate"];
    assert_eq!(u["userErrors"], json!([]));
    assert_eq!(u["order"]["tags"], json!(["VIP", "priority"]));
    assert_eq!(u["order"]["note"], "Leave with concierge");
    assert_eq!(
        u["order"]["metafields"]["edges"][0]["node"]["id"],
        "gid://shopify/Metafield/1003"
    );
    assert_eq!(u["order"]["shippingAddress"]["province"], "TX");
    let s = state(&w);
    let order = &s["orders"]["5502"];
    assert_eq!(order["note"], "Leave with concierge");
    assert_eq!(order["tags"], json!(["VIP", "priority"]));
    assert_eq!(order["shippingAddress"]["provinceCode"], "TX");
    assert_eq!(order["updatedAt"], "2026-05-04T15:00:07Z");

    let missing = gql(
        &app,
        "orderUpdate",
        json!({"input": {"id": "gid://shopify/Order/1", "note": "x"}}),
    )
    .await;
    assert_eq!(
        missing["data"]["orderUpdate"],
        json!({"order": null, "userErrors": [{"field": ["id"], "message": "Order does not exist"}]})
    );
    let unknown = gql(
        &app,
        "orderUpdate",
        json!({"input": {"id": "gid://shopify/Order/5502", "status": "closed"}}),
    )
    .await;
    assert_eq!(
        unknown["errors"][0]["message"],
        "Variable $input of type OrderInput! was provided invalid value for status (Field is not defined on OrderInput)"
    );
    let bad_mf = gql(&app, "orderUpdate", json!({"input": {"id": "gid://shopify/Order/5502", "metafields": [{"key": "x", "value": "1"}]}})).await;
    assert_eq!(
        bad_mf["data"]["orderUpdate"]["userErrors"][0]["message"],
        "Type can't be blank"
    );
    assert_eq!(
        state(&w)["orders"]["5502"]["metafields"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn customer_update_writes_state_and_validates() {
    let (w, app) = setup();
    let r = gql(&app, "customerUpdate", json!({"input": {
        "id": "gid://shopify/Customer/7002",
        "tags": ["VIP"],
        "note": "Prefers email",
        "phone": "(512) 555-0199",
        "taxExempt": true,
        "metafields": [{"namespace": "loyalty", "key": "tier", "value": "gold", "type": "single_line_text_field"}]
    }})).await;
    let c = &r["data"]["customerUpdate"]["customer"];
    assert_eq!(c["tags"], json!(["VIP"]));
    assert_eq!(c["phone"], "+15125550199");
    assert_eq!(c["taxExempt"], true);
    assert_eq!(c["metafields"]["edges"][0]["node"]["key"], "tier");
    assert_eq!(state(&w)["customers"]["7002"]["note"], "Prefers email");

    let taken = gql(
        &app,
        "customerUpdate",
        json!({"input": {"id": "gid://shopify/Customer/7002", "email": "sortiz@gmail.com"}}),
    )
    .await;
    assert_eq!(
        taken["data"]["customerUpdate"]["userErrors"],
        json!([{"field": ["email"], "message": "Email has already been taken"}])
    );
    let missing = gql(
        &app,
        "customerUpdate",
        json!({"input": {"id": "gid://shopify/Customer/1"}}),
    )
    .await;
    assert_eq!(
        missing["data"]["customerUpdate"]["userErrors"][0]["message"],
        "Customer does not exist"
    );
    let phone = gql(
        &app,
        "customerUpdate",
        json!({"input": {"id": "gid://shopify/Customer/7002", "phone": "call me"}}),
    )
    .await;
    assert_eq!(
        phone["data"]["customerUpdate"]["userErrors"][0]["field"],
        json!(["phone"])
    );
    assert_eq!(
        state(&w)["customers"]["7002"]["email"],
        "sam.ortiz@fastmail.com"
    );
}

#[tokio::test]
async fn product_create_update_and_delete() {
    let (w, app) = setup();
    let r = gql(&app, "productCreate", json!({"product": {
        "title": "Harbor Canvas Tote",
        "status": "DRAFT",
        "tags": ["bags"],
        "productOptions": [{"name": "Size", "values": [{"name": "Mini"}, {"name": "Large"}]}],
        "metafields": [{"namespace": "custom", "key": "care", "value": "Spot clean", "type": "single_line_text_field"}],
        "collectionsToJoin": ["gid://shopify/Collection/301"]
    }})).await;
    let p = &r["data"]["productCreate"]["product"];
    assert_eq!(r["data"]["productCreate"]["userErrors"], json!([]));
    assert_eq!(p["id"], "gid://shopify/Product/8104");
    assert_eq!(p["handle"], "harbor-canvas-tote-1");
    assert_eq!(p["vendor"], "Octo Corp");
    assert_eq!(
        p["options"],
        json!([{"id": "gid://shopify/ProductOption/53", "name": "Size", "values": ["Mini", "Large"]}])
    );
    let created = &state(&w)["products"]["8104"];
    assert_eq!(created["variants"][0]["title"], "Mini");
    assert_eq!(created["variants"][0]["price"], "0.00");
    assert_eq!(created["collections"], json!([301]));
    let blank = gql(&app, "productCreate", json!({"product": {"title": " "}})).await;
    assert_eq!(
        blank["data"]["productCreate"]["userErrors"][0]["message"],
        "Title can't be blank"
    );

    let u = gql(
        &app,
        "productUpdate",
        json!({"input": {
            "id": "gid://shopify/Product/8102",
            "status": "ACTIVE",
            "descriptionHtml": "<p>Now in charcoal.</p>",
            "seo": {"title": "Alpine Beanie"},
            "collectionsToJoin": ["gid://shopify/Collection/302"]
        }}),
    )
    .await;
    let up = &u["data"]["productUpdate"]["product"];
    assert_eq!(up["status"], "ACTIVE");
    assert_eq!(
        up["seo"],
        json!({"title": "Alpine Beanie", "description": null})
    );
    assert_eq!(up["variants"]["edges"][0]["node"]["options"], Value::Null);
    assert_eq!(
        up["variants"]["edges"][0]["node"]["selectedOptions"],
        json!([{"name": "Title", "value": "Default Title"}])
    );
    let s = state(&w);
    assert_eq!(s["products"]["8102"]["status"], "ACTIVE");
    assert_eq!(
        s["products"]["8102"]["publishedAt"],
        s["products"]["8102"]["updatedAt"]
    );
    assert_eq!(s["products"]["8102"]["collections"], json!([302]));
    let clash = gql(
        &app,
        "productUpdate",
        json!({"input": {"id": "gid://shopify/Product/8102", "handle": "linen-shirt"}}),
    )
    .await;
    assert_eq!(
        clash["data"]["productUpdate"]["userErrors"][0]["field"],
        json!(["handle"])
    );
    let missing = gql(
        &app,
        "productUpdate",
        json!({"input": {"id": "gid://shopify/Product/1", "title": "x"}}),
    )
    .await;
    assert_eq!(
        missing["data"]["productUpdate"]["userErrors"][0]["message"],
        "Product does not exist"
    );

    let d = gql(
        &app,
        "productDelete",
        json!({"input": {"id": "gid://shopify/Product/8104"}}),
    )
    .await;
    assert_eq!(
        d["data"]["productDelete"],
        json!({"deletedProductId": "gid://shopify/Product/8104", "userErrors": []})
    );
    assert!(state(&w)["products"].get("8104").is_none());
    let again = gql(
        &app,
        "productDelete",
        json!({"input": {"id": "gid://shopify/Product/8104"}}),
    )
    .await;
    assert_eq!(
        again["data"]["productDelete"]["deletedProductId"],
        Value::Null
    );
    assert_eq!(
        again["data"]["productDelete"]["userErrors"][0]["message"],
        "Product does not exist"
    );
}

#[tokio::test]
async fn variants_bulk_create_update_delete() {
    let (w, app) = setup();
    let created = gql(&app, "productVariantsBulkCreate", json!({
        "productId": "gid://shopify/Product/8103",
        "variants": [{"price": "62.00", "inventoryItem": {"sku": "LS-L"}, "optionValues": [{"optionName": "Size", "name": "L"}]}]
    })).await;
    let c = &created["data"]["productVariantsBulkCreate"];
    assert_eq!(c["userErrors"], json!([]));
    assert_eq!(
        c["productVariants"][0],
        json!({"id": "gid://shopify/ProductVariant/4106", "title": "L", "price": "62.00", "sku": "LS-L", "selectedOptions": [{"name": "Size", "value": "L"}]})
    );
    let dup = gql(
        &app,
        "productVariantsBulkCreate",
        json!({
            "productId": "gid://shopify/Product/8103",
            "variants": [{"optionValues": [{"optionName": "Size", "name": "S"}]}]
        }),
    )
    .await;
    assert_eq!(
        dup["data"]["productVariantsBulkCreate"]["userErrors"],
        json!([{"field": ["variants", "0"], "message": "The variant 'S' already exists."}])
    );
    let bad_option = gql(
        &app,
        "productVariantsBulkCreate",
        json!({
            "productId": "gid://shopify/Product/8103",
            "variants": [{"optionValues": [{"optionName": "Color", "name": "Red"}]}]
        }),
    )
    .await;
    assert_eq!(
        bad_option["data"]["productVariantsBulkCreate"]["userErrors"][0]["message"],
        "Option does not exist"
    );

    // A default-variant product gets real options, and the standalone variant goes away.
    let beanie = gql(
        &app,
        "productVariantsBulkCreate",
        json!({
            "productId": "gid://shopify/Product/8102",
            "strategy": "REMOVE_STANDALONE_VARIANT",
            "variants": [
                {"price": "24.00", "optionValues": [{"optionName": "Color", "name": "Charcoal"}]},
                {"price": "26.00", "optionValues": [{"optionName": "Color", "name": "Rust"}]}
            ]
        }),
    )
    .await;
    assert_eq!(
        beanie["data"]["productVariantsBulkCreate"]["userErrors"],
        json!([])
    );
    let s = state(&w);
    let variants = s["products"]["8102"]["variants"].as_array().unwrap();
    assert_eq!(
        variants
            .iter()
            .map(|v| v["title"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Charcoal", "Rust"]
    );
    assert_eq!(s["products"]["8102"]["options"][0]["name"], "Color");

    let updated = gql(&app, "productVariantsBulkUpdate", json!({
        "productId": "gid://shopify/Product/8101",
        "variants": [{"id": "gid://shopify/ProductVariant/4101", "price": "42", "compareAtPrice": "50.00"}]
    })).await;
    let u = &updated["data"]["productVariantsBulkUpdate"];
    assert_eq!(u["productVariants"][0]["price"], "42.00");
    let s = state(&w);
    assert_eq!(s["products"]["8101"]["variants"][0]["price"], "42.00");
    assert_eq!(
        s["products"]["8101"]["variants"][0]["compareAtPrice"],
        "50.00"
    );
    assert_eq!(s["products"]["8101"]["variants"][1]["price"], "38.00");
    let foreign = gql(
        &app,
        "productVariantsBulkUpdate",
        json!({
            "productId": "gid://shopify/Product/8101",
            "variants": [{"id": "gid://shopify/ProductVariant/4104", "price": "1.00"}]
        }),
    )
    .await;
    assert_eq!(
        foreign["data"]["productVariantsBulkUpdate"]["userErrors"][0]["message"],
        "Product variant does not exist"
    );
    assert_eq!(
        state(&w)["products"]["8103"]["variants"][0]["price"],
        "58.00"
    );

    let deleted = gql(&app, "productVariantsBulkDelete", json!({"productId": "gid://shopify/Product/8101", "variantsIds": ["gid://shopify/ProductVariant/4102"]})).await;
    let rem = &deleted["data"]["productVariantsBulkDelete"]["product"]["variants"]["edges"];
    assert_eq!(rem.as_array().unwrap().len(), 1);
    let bad = gql(&app, "productVariantsBulkDelete", json!({"productId": "gid://shopify/Product/8101", "variantsIds": ["gid://shopify/ProductVariant/4104"]})).await;
    assert_eq!(
        bad["data"]["productVariantsBulkDelete"]["userErrors"][0]["message"],
        "At least one variant does not belong to the product"
    );
    let no_product = gql(&app, "productVariantsBulkDelete", json!({"productId": "gid://shopify/Product/1", "variantsIds": ["gid://shopify/ProductVariant/4104"]})).await;
    assert_eq!(
        no_product["data"]["productVariantsBulkDelete"]["userErrors"][0]["message"],
        "Product does not exist"
    );
}

#[tokio::test]
async fn product_options_create_update_delete() {
    let (w, app) = setup();
    let created = gql(
        &app,
        "productOptionsCreate",
        json!({
            "productId": "gid://shopify/Product/8101",
            "options": [{"name": "Size", "values": [{"name": "Regular"}, {"name": "Mini"}]}]
        }),
    )
    .await;
    let c = &created["data"]["productOptionsCreate"];
    assert_eq!(c["userErrors"], json!([]));
    let opts = c["product"]["options"].as_array().unwrap();
    assert_eq!(opts[1]["name"], "Size");
    assert_eq!(opts[1]["optionValues"][0]["hasVariants"], true);
    assert_eq!(opts[1]["optionValues"][1]["hasVariants"], false);
    assert_eq!(
        c["product"]["variants"]["edges"][0]["node"]["title"],
        "Navy / Regular"
    );
    let dup = gql(&app, "productOptionsCreate", json!({"productId": "gid://shopify/Product/8101", "options": [{"name": "color", "values": [{"name": "Red"}]}]})).await;
    assert_eq!(
        dup["data"]["productOptionsCreate"]["userErrors"][0]["code"],
        "OPTION_ALREADY_EXISTS"
    );

    let size_id = opts[1]["id"].as_str().unwrap().to_owned();
    let mini_id = opts[1]["optionValues"][1]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let updated = gql(
        &app,
        "productOptionUpdate",
        json!({
            "productId": "gid://shopify/Product/8101",
            "option": {"id": size_id, "name": "Format", "position": 1},
            "optionValuesToAdd": [{"name": "Weekender"}],
            "optionValuesToDelete": [mini_id]
        }),
    )
    .await;
    let u = &updated["data"]["productOptionUpdate"];
    assert_eq!(u["userErrors"], json!([]));
    assert_eq!(u["product"]["options"][0]["name"], "Format");
    assert_eq!(
        u["product"]["options"][0]["optionValues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Regular", "Weekender"]
    );
    assert_eq!(
        u["product"]["variants"]["edges"][0]["node"]["title"],
        "Regular / Navy"
    );
    let regular_id = u["product"]["options"][0]["optionValues"][0]["id"].clone();
    let in_use = gql(&app, "productOptionUpdate", json!({"productId": "gid://shopify/Product/8101", "option": {"id": size_id}, "optionValuesToDelete": [regular_id]})).await;
    assert_eq!(
        in_use["data"]["productOptionUpdate"]["userErrors"][0]["code"],
        "OPTION_VALUE_HAS_VARIANTS"
    );

    let deleted = gql(
        &app,
        "productOptionsDelete",
        json!({"productId": "gid://shopify/Product/8101", "options": [size_id]}),
    )
    .await;
    let d = &deleted["data"]["productOptionsDelete"];
    assert_eq!(d["userErrors"], json!([]));
    assert_eq!(d["product"]["options"].as_array().unwrap().len(), 1);
    assert_eq!(
        state(&w)["products"]["8101"]["variants"][0]["title"],
        "Navy"
    );
    let collide = gql(&app, "productOptionsDelete", json!({"productId": "gid://shopify/Product/8103", "options": ["gid://shopify/ProductOption/52"]})).await;
    assert_eq!(
        collide["data"]["productOptionsDelete"]["userErrors"][0]["code"],
        "CANNOT_DELETE_OPTION_WITH_MULTIPLE_VALUES"
    );
    let missing = gql(&app, "productOptionsDelete", json!({"productId": "gid://shopify/Product/8103", "options": ["gid://shopify/ProductOption/1"]})).await;
    assert_eq!(
        missing["data"]["productOptionsDelete"]["userErrors"][0]["code"],
        "OPTION_DOES_NOT_EXIST"
    );
}

#[tokio::test]
async fn graphql_errors_look_like_shopify() {
    let (_, app) = setup();
    let undefined = gql_raw(
        &app,
        "query Q {\n  products(first: 1) {\n    edges { node { price } }\n  }\n}",
        json!({}),
    )
    .await;
    assert_eq!(
        undefined,
        json!({"errors": [{
            "message": "Field 'price' doesn't exist on type 'Product'",
            "locations": [{"line": 3, "column": 20}],
            "path": ["query Q", "products", "edges", "node", "price"],
            "extensions": {"code": "undefinedField", "typeName": "Product", "fieldName": "price"}
        }]})
    );
    let syntax = gql_raw(&app, "query { products(first: 1) { edges }", json!({})).await;
    assert!(
        syntax["errors"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("Parse error on")
    );
    let unknown_root = gql_raw(&app, "{ kittens { id } }", json!({})).await;
    assert_eq!(
        unknown_root["errors"][0]["extensions"]["typeName"],
        "QueryRoot"
    );
    let no_sel = gql_raw(&app, "{ shop }", json!({})).await;
    assert_eq!(
        no_sel["errors"][0]["extensions"]["code"],
        "selectionMismatch"
    );
    let aliased = gql_raw(&app, "{ s: shop { name domain: myshopifyDomain } p: product(id: \"gid://shopify/Product/8102\") { ... on Product { title } __typename } }", json!({})).await;
    assert_eq!(
        aliased["data"],
        json!({"s": {"name": "Octo Corp", "domain": "octo-corp.myshopify.com"}, "p": {"title": "Alpine Wool Beanie", "__typename": "Product"}})
    );
    let missing_var = gql(&app, "GetProducts", json!({})).await;
    assert_eq!(
        missing_var["errors"][0]["message"],
        "Variable $first of type Int! was provided invalid value"
    );
}

#[tokio::test]
async fn queries_do_not_change_state_and_mutations_are_deterministic() {
    let run = || async {
        let (w, app) = setup();
        gql(&app, "GetOrders", json!({"first": 10})).await;
        gql(
            &app,
            "productCreate",
            json!({"product": {"title": "Tide Pool Mug", "status": "ACTIVE"}}),
        )
        .await;
        state(&w)
    };
    let a = run().await;
    let b = run().await;
    assert_eq!(a, b);
    assert_eq!(a["products"]["8104"]["createdAt"], "2026-05-04T15:00:14Z");
    assert_eq!(a["products"]["8104"]["handle"], "tide-pool-mug");
}
