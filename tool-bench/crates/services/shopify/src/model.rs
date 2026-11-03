//! Store documents and their GraphQL object views.
//!
//! Dataset collections (all keyed by the numeric id):
//! - `products`: `{id, title, handle, descriptionHtml, vendor, productType, status, tags,
//!   createdAt, updatedAt, publishedAt, seo{title,description}, options[{id,name,position,
//!   values[{id,name}]}], variants[{id,title,price,compareAtPrice,sku,barcode,taxable,tracked,
//!   inventoryQuantity,selectedOptions[{name,value}],createdAt,updatedAt}], images[{id,url,
//!   altText,width,height}], collections[<collection id>], metafields[...]}`
//! - `orders`: `{id, name, email, phone, customerId, createdAt, processedAt, updatedAt,
//!   closedAt, cancelledAt, cancelReason, financialStatus, fulfillmentStatus, currencyCode,
//!   subtotalPrice, totalShipping, totalTax, totalDiscounts, totalPrice, shippingAddress,
//!   billingAddress, lineItems[{id,title,quantity,price,variantId,productId,sku,variantTitle,
//!   vendor}], tags, note, customAttributes[{key,value}], metafields[...]}`
//! - `customers`: `{id, firstName, lastName, email, phone, createdAt, updatedAt, tags, note,
//!   taxExempt, state, addresses[MailingAddress], metafields[...]}`
//! - `collections`: `{id, title, handle, descriptionHtml, updatedAt}`
//! - `shop` (optional, key `shop`): `{name, email, currencyCode, ianaTimezone}`
//!
//! Metafields are `{id, namespace, key, value, type, createdAt, updatedAt}`. Money amounts
//! are decimal strings. Object views use three markers resolved by the executor:
//! `{"__ref": [type, id]}`, `{"__connection": type, "items": [...]}` and
//! `{"__metafield": [...]}` (the `metafield(namespace:, key:)` lookup).

use mock_core::store::Store;
use serde_json::{Value, json};

pub const DOMAIN: &str = "octo-corp.myshopify.com";

pub fn gid(kind: &str, id: u64) -> String {
    format!("gid://shopify/{kind}/{id}")
}

/// `gid://shopify/Product/123?x=y` -> `("Product", 123)`.
pub fn parse_gid(value: &str) -> Option<(&str, u64)> {
    let rest = value.strip_prefix("gid://shopify/")?;
    let (kind, id) = rest.split_once('/')?;
    let id = id.split('?').next()?;
    Some((kind, id.parse().ok()?))
}

pub fn is_gid(value: &str) -> bool {
    value
        .strip_prefix("gid://shopify/")
        .and_then(|r| r.split_once('/'))
        .is_some_and(|(k, id)| !k.is_empty() && !id.is_empty())
}

pub fn reference(kind: &str, id: u64) -> Value {
    json!({ "__ref": [kind, id] })
}

pub fn connection(kind: &str, items: Vec<Value>) -> Value {
    json!({ "__connection": kind, "items": items })
}

pub fn id_of(doc: &Value) -> u64 {
    match &doc["id"] {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::String(s) => s
            .parse()
            .ok()
            .or_else(|| parse_gid(s).map(|(_, id)| id))
            .unwrap_or(0),
        _ => 0,
    }
}

pub fn docs<'a>(store: &'a Store, collection: &str) -> Vec<&'a Value> {
    store
        .collection(collection)
        .map(|c| c.values().collect())
        .unwrap_or_default()
}

pub fn find<'a>(store: &'a Store, collection: &str, id: u64) -> Option<&'a Value> {
    let c = store.collection(collection)?;
    c.get(&id.to_string())
        .filter(|d| id_of(d) == id)
        .or_else(|| c.values().find(|d| id_of(d) == id))
}

/// The store key of the document with this id.
pub fn key_of(store: &Store, collection: &str, id: u64) -> Option<String> {
    let c = store.collection(collection)?;
    c.iter()
        .find(|(_, d)| id_of(d) == id)
        .map(|(k, _)| k.to_owned())
}

pub fn str_of(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

pub fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub fn shop(store: &Store) -> Value {
    let doc = store
        .collection("shop")
        .and_then(|c| c.values().next())
        .cloned()
        .unwrap_or(Value::Null);
    let text = |k: &str, d: &str| doc[k].as_str().unwrap_or(d).to_owned();
    json!({
        "__typename": "Shop",
        "id": gid("Shop", doc["id"].as_u64().unwrap_or(1)),
        "name": text("name", "Octo Corp"),
        "email": text("email", "ops@octo-corp.example"),
        "contactEmail": text("email", "ops@octo-corp.example"),
        "myshopifyDomain": DOMAIN,
        "url": format!("https://{DOMAIN}"),
        "currencyCode": currency(store),
        "ianaTimezone": text("ianaTimezone", "America/New_York"),
        "timezoneAbbreviation": text("timezoneAbbreviation", "EST"),
        "primaryDomain": {"__typename": "Domain", "host": DOMAIN, "url": format!("https://{DOMAIN}"), "sslEnabled": true},
        "plan": {"__typename": "ShopPlan", "displayName": "Basic", "partnerDevelopment": false, "shopifyPlus": false},
    })
}

pub fn currency(store: &Store) -> String {
    store
        .collection("shop")
        .and_then(|c| c.values().next())
        .and_then(|s| s["currencyCode"].as_str())
        .unwrap_or("USD")
        .to_owned()
}

// ---------------------------------------------------------------------------------------
// Money

/// Parses a decimal amount (string or number) into cents.
pub fn cents(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .map(|i| i * 100)
            .or_else(|| n.as_f64().map(|f| (f * 100.0).round() as i64)),
        Value::String(s) => parse_cents(s),
        _ => None,
    }
}

pub fn parse_cents(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty() && frac.is_empty()
        || !whole.chars().all(|c| c.is_ascii_digit())
        || !frac.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut frac_digits: String = frac.chars().take(3).collect();
    while frac_digits.len() < 3 {
        frac_digits.push('0');
    }
    let thousandths: i64 = frac_digits.parse().ok()?;
    let value = whole * 100 + (thousandths + 5) / 10;
    Some(if neg { -value } else { value })
}

/// `Money` scalar form: `"12.30"`.
pub fn money_str(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let c = cents.abs();
    format!("{sign}{}.{:02}", c / 100, c % 100)
}

/// `Decimal` scalar form used in `MoneyV2.amount`: `"12.3"`, `"12.0"`.
pub fn decimal_str(cents: i64) -> String {
    let s = money_str(cents);
    let trimmed = s.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{trimmed}0")
    } else {
        trimmed.to_owned()
    }
}

pub fn money(cents: i64, currency: &str) -> Value {
    json!({ "__typename": "MoneyV2", "amount": decimal_str(cents), "currencyCode": currency })
}

pub fn money_bag(cents: i64, currency: &str) -> Value {
    json!({
        "__typename": "MoneyBag",
        "shopMoney": money(cents, currency),
        "presentmentMoney": money(cents, currency),
    })
}

// ---------------------------------------------------------------------------------------
// Shared pieces

pub fn metafield(doc: &Value, owner: &str) -> Value {
    json!({
        "__typename": "Metafield",
        "id": gid("Metafield", id_of(doc)),
        "legacyResourceId": id_of(doc).to_string(),
        "namespace": doc["namespace"],
        "key": doc["key"],
        "value": doc["value"],
        "type": doc["type"],
        "description": doc.get("description").cloned().unwrap_or(Value::Null),
        "ownerType": owner,
        "createdAt": doc["createdAt"],
        "updatedAt": doc["updatedAt"],
        "jsonValue": metafield_json_value(doc),
    })
}

fn metafield_json_value(doc: &Value) -> Value {
    let raw = doc["value"].as_str().unwrap_or_default();
    let ty = doc["type"].as_str().unwrap_or_default();
    if ty == "json" || ty.starts_with("list.") {
        serde_json::from_str(raw).unwrap_or(Value::String(raw.to_owned()))
    } else if ty == "number_integer" {
        raw.parse::<i64>()
            .map(Value::from)
            .unwrap_or(Value::String(raw.to_owned()))
    } else if ty == "boolean" {
        Value::Bool(raw == "true")
    } else {
        Value::String(raw.to_owned())
    }
}

fn metafields(doc: &Value, owner: &str) -> (Value, Value) {
    let list: Vec<Value> = doc["metafields"]
        .as_array()
        .map(|a| a.iter().map(|m| metafield(m, owner)).collect())
        .unwrap_or_default();
    (
        connection("Metafield", list.clone()),
        json!({ "__metafield": list }),
    )
}

pub fn address(doc: &Value) -> Value {
    if !doc.is_object() {
        return Value::Null;
    }
    let s = |k: &str| doc.get(k).cloned().unwrap_or(Value::Null);
    let first = doc["firstName"].as_str().unwrap_or_default();
    let last = doc["lastName"].as_str().unwrap_or_default();
    let name = format!("{first} {last}").trim().to_owned();
    let country_code = doc
        .get("countryCode")
        .or_else(|| doc.get("countryCodeV2"))
        .cloned()
        .unwrap_or(Value::Null);
    let formatted: Vec<Value> = [
        doc["address1"].as_str().map(str::to_owned),
        doc["address2"].as_str().map(str::to_owned),
        Some(
            [
                doc["city"].as_str(),
                doc["provinceCode"].as_str().or(doc["province"].as_str()),
                doc["zip"].as_str(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
        ),
        doc["country"].as_str().map(str::to_owned),
    ]
    .into_iter()
    .flatten()
    .filter(|l| !l.is_empty())
    .map(Value::String)
    .collect();
    json!({
        "__typename": "MailingAddress",
        "id": doc.get("id").map(|id| match id {
            Value::Number(n) => Value::String(format!("gid://shopify/MailingAddress/{n}?model_name=CustomerAddress")),
            other => other.clone(),
        }).unwrap_or(Value::Null),
        "address1": s("address1"),
        "address2": s("address2"),
        "city": s("city"),
        "company": s("company"),
        "country": s("country"),
        "countryCodeV2": country_code,
        "firstName": s("firstName"),
        "lastName": s("lastName"),
        "name": if name.is_empty() { Value::Null } else { Value::String(name) },
        "phone": s("phone"),
        "province": s("province"),
        "provinceCode": s("provinceCode"),
        "zip": s("zip"),
        "formatted": formatted,
        "formattedArea": Value::Null,
    })
}

/// Plain-text `description` from `descriptionHtml`.
pub fn strip_html(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let closing = tag.starts_with('/');
                if name == "br"
                    || (closing
                        && matches!(
                            name.as_str(),
                            "p" | "li" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                        ))
                {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(c),
            _ => out.push(c),
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let lines: Vec<&str> = decoded
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

// ---------------------------------------------------------------------------------------
// Products

pub fn is_default_option(options: &[Value]) -> bool {
    options.len() == 1
        && options[0]["name"] == "Title"
        && options[0]["values"]
            .as_array()
            .is_some_and(|v| v.len() == 1 && v[0]["name"] == "Default Title")
}

pub fn default_option(option_id: u64, value_id: u64) -> Value {
    json!({"id": option_id, "name": "Title", "position": 1, "values": [{"id": value_id, "name": "Default Title"}]})
}

pub fn variant_title(selected: &[Value]) -> String {
    if selected.is_empty() {
        return "Default Title".to_owned();
    }
    selected
        .iter()
        .filter_map(|s| s["value"].as_str())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Fills in defaults so every product document has options, variant titles and
/// selected options. Applied before every product mutation.
pub fn normalize_product(doc: &mut Value) {
    let pid = id_of(doc);
    if !doc["variants"].is_array() {
        doc["variants"] = json!([]);
    }
    if !doc["options"].is_array() || doc["options"].as_array().is_some_and(Vec::is_empty) {
        doc["options"] = json!([default_option(pid, pid)]);
    }
    let options = doc["options"].as_array().cloned().unwrap_or_default();
    let default = is_default_option(&options);
    for (n, opt) in doc["options"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if opt["position"].is_null() {
            opt["position"] = json!(n + 1);
        }
    }
    for v in doc["variants"].as_array_mut().into_iter().flatten() {
        if !v["selectedOptions"].is_array() {
            v["selectedOptions"] = if default {
                json!([{"name": "Title", "value": "Default Title"}])
            } else {
                json!([])
            };
        }
        if v["title"].is_null() {
            let selected = v["selectedOptions"].as_array().cloned().unwrap_or_default();
            v["title"] = json!(variant_title(&selected));
        }
    }
}

pub fn product(store: &Store, raw: &Value) -> Value {
    let mut doc = raw.clone();
    normalize_product(&mut doc);
    let id = id_of(&doc);
    let cur = currency(store);
    let variants_docs = doc["variants"].as_array().cloned().unwrap_or_default();
    let variants: Vec<Value> = variants_docs
        .iter()
        .map(|v| variant(store, &doc, v))
        .collect();
    let prices: Vec<i64> = variants_docs
        .iter()
        .filter_map(|v| cents(&v["price"]))
        .collect();
    let compare: Vec<i64> = variants_docs
        .iter()
        .filter_map(|v| cents(&v["compareAtPrice"]))
        .collect();
    let min = prices.iter().copied().min().unwrap_or(0);
    let max = prices.iter().copied().max().unwrap_or(0);
    let total_inventory: i64 = variants_docs
        .iter()
        .filter_map(|v| v["inventoryQuantity"].as_i64())
        .sum();
    let options: Vec<Value> = doc["options"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|o| product_option(o, &variants_docs))
        .collect();
    let images: Vec<Value> = doc["images"]
        .as_array()
        .map(|a| a.iter().map(image).collect())
        .unwrap_or_default();
    let collections: Vec<Value> = doc["collections"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .filter(|cid| find(store, "collections", *cid).is_some())
                .map(|cid| reference("Collection", cid))
                .collect()
        })
        .unwrap_or_default();
    let (metafields, metafield) = metafields(&doc, "PRODUCT");
    let html = doc["descriptionHtml"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let handle = doc["handle"].as_str().unwrap_or_default().to_owned();
    let status = doc["status"].as_str().unwrap_or("ACTIVE").to_owned();
    let seo = &doc["seo"];
    json!({
        "__typename": "Product",
        "id": gid("Product", id),
        "legacyResourceId": id.to_string(),
        "title": doc["title"],
        "description": strip_html(&html),
        "descriptionHtml": html,
        "handle": handle,
        "status": status,
        "vendor": doc["vendor"].as_str().unwrap_or_default(),
        "productType": doc["productType"].as_str().unwrap_or_default(),
        "tags": doc["tags"].as_array().cloned().unwrap_or_default(),
        "createdAt": doc["createdAt"],
        "updatedAt": doc.get("updatedAt").filter(|v| !v.is_null()).unwrap_or(&doc["createdAt"]),
        "publishedAt": doc.get("publishedAt").cloned().unwrap_or(Value::Null),
        "isGiftCard": false,
        "totalInventory": total_inventory,
        "tracksInventory": variants_docs.iter().any(|v| v["tracked"].as_bool().unwrap_or(true)),
        "hasOnlyDefaultVariant": is_default_option(doc["options"].as_array().map(Vec::as_slice).unwrap_or_default()),
        "hasOutOfStockVariants": variants_docs.iter().any(|v| v["inventoryQuantity"].as_i64().unwrap_or(0) <= 0),
        "onlineStoreUrl": if status == "ACTIVE" { Value::String(format!("https://{DOMAIN}/products/{handle}")) } else { Value::Null },
        "onlineStorePreviewUrl": format!("https://{DOMAIN}/products/{handle}"),
        "templateSuffix": Value::Null,
        "seo": {"__typename": "SEO", "title": seo.get("title").cloned().unwrap_or(Value::Null), "description": seo.get("description").cloned().unwrap_or(Value::Null)},
        "priceRangeV2": {"__typename": "ProductPriceRangeV2", "minVariantPrice": money(min, &cur), "maxVariantPrice": money(max, &cur)},
        "priceRange": {"__typename": "ProductPriceRange", "minVariantPrice": money(min, &cur), "maxVariantPrice": money(max, &cur)},
        "compareAtPriceRange": if compare.is_empty() { Value::Null } else {
            json!({"__typename": "ProductCompareAtPriceRange", "minVariantCompareAtPrice": money(*compare.iter().min().unwrap_or(&0), &cur), "maxVariantCompareAtPrice": money(*compare.iter().max().unwrap_or(&0), &cur)})
        },
        "options": options,
        "variants": connection("ProductVariant", variants),
        "variantsCount": {"__typename": "Count", "count": variants_docs.len(), "precision": "EXACT"},
        "totalVariants": variants_docs.len(),
        "images": connection("Image", images.clone()),
        "featuredImage": images.first().cloned().unwrap_or(Value::Null),
        "collections": connection("Collection", collections),
        "metafields": metafields,
        "metafield": metafield,
    })
}

fn product_option(option: &Value, variants: &[Value]) -> Value {
    let name = option["name"].as_str().unwrap_or_default();
    let values: Vec<Value> = option["values"].as_array().cloned().unwrap_or_default();
    let option_values: Vec<Value> = values
        .iter()
        .map(|v| {
            let has_variants = variants.iter().any(|variant| {
                variant["selectedOptions"].as_array().is_some_and(|s| {
                    s.iter()
                        .any(|so| so["name"] == name && so["value"] == v["name"])
                })
            });
            json!({
                "__typename": "ProductOptionValue",
                "id": gid("ProductOptionValue", id_of(v)),
                "name": v["name"],
                "hasVariants": has_variants,
            })
        })
        .collect();
    json!({
        "__typename": "ProductOption",
        "id": gid("ProductOption", id_of(option)),
        "name": name,
        "position": option["position"],
        "values": values.iter().map(|v| v["name"].clone()).collect::<Vec<_>>(),
        "optionValues": option_values,
    })
}

fn image(doc: &Value) -> Value {
    json!({
        "__typename": "Image",
        "id": gid("ProductImage", id_of(doc)),
        "url": doc["url"],
        "src": doc["url"],
        "altText": doc.get("altText").cloned().unwrap_or(Value::Null),
        "width": doc.get("width").cloned().unwrap_or(Value::Null),
        "height": doc.get("height").cloned().unwrap_or(Value::Null),
    })
}

pub fn variant(store: &Store, product: &Value, doc: &Value) -> Value {
    let id = id_of(doc);
    let cur = currency(store);
    let price = cents(&doc["price"]).unwrap_or(0);
    let compare = cents(&doc["compareAtPrice"]);
    let selected: Vec<Value> = doc["selectedOptions"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![json!({"name": "Title", "value": "Default Title"})])
        .into_iter()
        .map(|s| json!({"__typename": "SelectedOption", "name": s["name"], "value": s["value"]}))
        .collect();
    let title = doc["title"].as_str().map(str::to_owned).unwrap_or_else(|| {
        variant_title(
            doc["selectedOptions"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
        )
    });
    let position = product["variants"]
        .as_array()
        .and_then(|vs| vs.iter().position(|v| id_of(v) == id))
        .map_or(1, |p| p + 1);
    let quantity = doc["inventoryQuantity"].as_i64().unwrap_or(0);
    let tracked = doc["tracked"].as_bool().unwrap_or(true);
    let sku = doc.get("sku").cloned().unwrap_or(Value::Null);
    let product_title = product["title"].as_str().unwrap_or_default();
    let (metafields, metafield) = metafields(doc, "PRODUCTVARIANT");
    json!({
        "__typename": "ProductVariant",
        "id": gid("ProductVariant", id),
        "legacyResourceId": id.to_string(),
        "title": title,
        "displayName": format!("{product_title} - {title}"),
        "price": money_str(price),
        "compareAtPrice": compare.map(money_str),
        "sku": sku,
        "barcode": doc.get("barcode").cloned().unwrap_or(Value::Null),
        "taxable": doc["taxable"].as_bool().unwrap_or(true),
        "inventoryQuantity": quantity,
        "availableForSale": !tracked || quantity > 0,
        "inventoryPolicy": doc["inventoryPolicy"].as_str().unwrap_or("DENY"),
        "position": position,
        "selectedOptions": selected,
        "createdAt": doc.get("createdAt").filter(|v| !v.is_null()).unwrap_or(&product["createdAt"]),
        "updatedAt": doc.get("updatedAt").filter(|v| !v.is_null()).or(doc.get("createdAt").filter(|v| !v.is_null())).unwrap_or(&product["createdAt"]),
        "inventoryItem": {
            "__typename": "InventoryItem",
            "id": gid("InventoryItem", id),
            "sku": sku,
            "tracked": tracked,
            "requiresShipping": true,
            "unitCost": Value::Null,
        },
        "product": reference("Product", id_of(product)),
        "image": Value::Null,
        "metafields": metafields,
        "metafield": metafield,
        "priceV2": money(price, &cur),
    })
}

/// The product owning a variant id.
pub fn variant_owner(store: &Store, variant_id: u64) -> Option<&Value> {
    docs(store, "products").into_iter().find(|p| {
        p["variants"]
            .as_array()
            .is_some_and(|vs| vs.iter().any(|v| id_of(v) == variant_id))
    })
}

// ---------------------------------------------------------------------------------------
// Orders

pub struct OrderTotals {
    pub subtotal: i64,
    pub shipping: i64,
    pub tax: i64,
    pub discounts: i64,
    pub total: i64,
}

pub fn order_totals(doc: &Value) -> OrderTotals {
    let line_sum: i64 = doc["lineItems"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|li| cents(&li["price"]).unwrap_or(0) * li["quantity"].as_i64().unwrap_or(1))
                .sum()
        })
        .unwrap_or(0);
    let discounts = cents(&doc["totalDiscounts"]).unwrap_or(0);
    let subtotal = cents(&doc["subtotalPrice"]).unwrap_or(line_sum - discounts);
    let shipping = cents(&doc["totalShipping"]).unwrap_or(0);
    let tax = cents(&doc["totalTax"]).unwrap_or(0);
    let total = cents(&doc["totalPrice"]).unwrap_or(subtotal + shipping + tax);
    OrderTotals {
        subtotal,
        shipping,
        tax,
        discounts,
        total,
    }
}

pub fn processed_at(doc: &Value) -> String {
    doc["processedAt"]
        .as_str()
        .or(doc["createdAt"].as_str())
        .unwrap_or_default()
        .to_owned()
}

pub fn order(store: &Store, doc: &Value) -> Value {
    let id = id_of(doc);
    let cur = doc["currencyCode"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| currency(store));
    let totals = order_totals(doc);
    let customer = doc["customerId"]
        .as_u64()
        .filter(|cid| find(store, "customers", *cid).is_some())
        .map_or(Value::Null, |cid| reference("Customer", cid));
    let line_items: Vec<Value> = doc["lineItems"]
        .as_array()
        .map(|items| items.iter().map(|li| line_item(store, li, &cur)).collect())
        .unwrap_or_default();
    let quantity: i64 = doc["lineItems"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|li| li["quantity"].as_i64().unwrap_or(1))
                .sum()
        })
        .unwrap_or(0);
    let financial = doc["financialStatus"].as_str().unwrap_or("PAID").to_owned();
    let (metafields, metafield) = metafields(doc, "ORDER");
    let closed_at = doc.get("closedAt").cloned().unwrap_or(Value::Null);
    let attributes: Vec<Value> = doc["customAttributes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(
                    |kv| json!({"__typename": "Attribute", "key": kv["key"], "value": kv["value"]}),
                )
                .collect()
        })
        .unwrap_or_default();
    json!({
        "__typename": "Order",
        "id": gid("Order", id),
        "legacyResourceId": id.to_string(),
        "name": doc["name"],
        "email": doc.get("email").cloned().unwrap_or(Value::Null),
        "phone": doc.get("phone").cloned().unwrap_or(Value::Null),
        "createdAt": doc["createdAt"],
        "processedAt": processed_at(doc),
        "updatedAt": doc.get("updatedAt").filter(|v| !v.is_null()).unwrap_or(&doc["createdAt"]),
        "closed": !closed_at.is_null(),
        "closedAt": closed_at,
        "cancelledAt": doc.get("cancelledAt").cloned().unwrap_or(Value::Null),
        "cancelReason": doc.get("cancelReason").cloned().unwrap_or(Value::Null),
        "confirmed": true,
        "test": false,
        "displayFinancialStatus": financial,
        "displayFulfillmentStatus": doc["fulfillmentStatus"].as_str().unwrap_or("UNFULFILLED"),
        "fullyPaid": financial == "PAID",
        "unpaid": matches!(financial.as_str(), "PENDING" | "AUTHORIZED" | "EXPIRED"),
        "currencyCode": cur,
        "presentmentCurrencyCode": cur,
        "subtotalPriceSet": money_bag(totals.subtotal, &cur),
        "currentSubtotalPriceSet": money_bag(totals.subtotal, &cur),
        "totalShippingPriceSet": money_bag(totals.shipping, &cur),
        "totalTaxSet": money_bag(totals.tax, &cur),
        "currentTotalTaxSet": money_bag(totals.tax, &cur),
        "totalDiscountsSet": money_bag(totals.discounts, &cur),
        "currentTotalDiscountsSet": money_bag(totals.discounts, &cur),
        "totalPriceSet": money_bag(totals.total, &cur),
        "currentTotalPriceSet": money_bag(totals.total, &cur),
        "totalRefundedSet": money_bag(if financial == "REFUNDED" { totals.total } else { 0 }, &cur),
        "subtotalLineItemsQuantity": quantity,
        "customer": customer,
        "shippingAddress": address(&doc["shippingAddress"]),
        "billingAddress": address(&doc["billingAddress"]),
        "lineItems": connection("LineItem", line_items),
        "tags": doc["tags"].as_array().cloned().unwrap_or_default(),
        "note": doc.get("note").cloned().unwrap_or(Value::Null),
        "customAttributes": attributes,
        "metafields": metafields,
        "metafield": metafield,
        "sourceName": "web",
        "statusPageUrl": format!("https://{DOMAIN}/orders/{id}/authenticate"),
    })
}

fn line_item(store: &Store, doc: &Value, cur: &str) -> Value {
    let id = id_of(doc);
    let qty = doc["quantity"].as_i64().unwrap_or(1);
    let unit = cents(&doc["price"]).unwrap_or(0);
    let variant = doc["variantId"]
        .as_u64()
        .filter(|vid| variant_owner(store, *vid).is_some())
        .map_or(Value::Null, |vid| reference("ProductVariant", vid));
    let product = doc["productId"]
        .as_u64()
        .filter(|pid| find(store, "products", *pid).is_some())
        .map_or(Value::Null, |pid| reference("Product", pid));
    let title = doc["title"].as_str().unwrap_or_default();
    let variant_title = doc.get("variantTitle").cloned().unwrap_or(Value::Null);
    let name = match variant_title.as_str() {
        Some(vt) if !vt.is_empty() && vt != "Default Title" => format!("{title} - {vt}"),
        _ => title.to_owned(),
    };
    json!({
        "__typename": "LineItem",
        "id": gid("LineItem", id),
        "title": title,
        "name": name,
        "quantity": qty,
        "currentQuantity": qty,
        "sku": doc.get("sku").cloned().unwrap_or(Value::Null),
        "variantTitle": variant_title,
        "vendor": doc.get("vendor").cloned().unwrap_or(Value::Null),
        "requiresShipping": true,
        "taxable": true,
        "originalUnitPriceSet": money_bag(unit, cur),
        "discountedUnitPriceSet": money_bag(unit, cur),
        "originalTotalSet": money_bag(unit * qty, cur),
        "discountedTotalSet": money_bag(unit * qty, cur),
        "variant": variant,
        "product": product,
    })
}

/// Orders of a customer, oldest first by processing time.
pub fn customer_orders(store: &Store, customer_id: u64) -> Vec<&Value> {
    let mut orders: Vec<&Value> = docs(store, "orders")
        .into_iter()
        .filter(|o| o["customerId"].as_u64() == Some(customer_id))
        .collect();
    orders.sort_by(|a, b| {
        processed_at(a)
            .cmp(&processed_at(b))
            .then(id_of(a).cmp(&id_of(b)))
    });
    orders
}

/// Total of the customer's orders that were not cancelled, voided or refunded.
pub fn amount_spent(store: &Store, customer_id: u64) -> i64 {
    customer_orders(store, customer_id)
        .into_iter()
        .filter(|o| o["cancelledAt"].is_null())
        .filter(|o| !matches!(o["financialStatus"].as_str(), Some("VOIDED" | "REFUNDED")))
        .map(|o| order_totals(o).total)
        .sum()
}

// ---------------------------------------------------------------------------------------
// Customers

pub fn customer(store: &Store, doc: &Value) -> Value {
    let id = id_of(doc);
    let cur = currency(store);
    let orders = customer_orders(store, id);
    let addresses: Vec<Value> = doc["addresses"]
        .as_array()
        .map(|a| a.iter().map(address).collect())
        .unwrap_or_default();
    let first = doc["firstName"].as_str().unwrap_or_default();
    let last = doc["lastName"].as_str().unwrap_or_default();
    let display = {
        let n = format!("{first} {last}").trim().to_owned();
        if n.is_empty() {
            doc["email"].as_str().unwrap_or_default().to_owned()
        } else {
            n
        }
    };
    let (metafields, metafield) = metafields(doc, "CUSTOMER");
    json!({
        "__typename": "Customer",
        "id": gid("Customer", id),
        "legacyResourceId": id.to_string(),
        "firstName": doc.get("firstName").cloned().unwrap_or(Value::Null),
        "lastName": doc.get("lastName").cloned().unwrap_or(Value::Null),
        "displayName": display,
        "email": doc.get("email").cloned().unwrap_or(Value::Null),
        "phone": doc.get("phone").cloned().unwrap_or(Value::Null),
        "createdAt": doc["createdAt"],
        "updatedAt": doc.get("updatedAt").filter(|v| !v.is_null()).unwrap_or(&doc["createdAt"]),
        "tags": doc["tags"].as_array().cloned().unwrap_or_default(),
        "note": doc.get("note").cloned().unwrap_or(Value::Null),
        "taxExempt": doc["taxExempt"].as_bool().unwrap_or(false),
        "taxExemptions": [],
        "state": doc["state"].as_str().unwrap_or("ENABLED"),
        "locale": doc["locale"].as_str().unwrap_or("en"),
        "verifiedEmail": true,
        "validEmailAddress": doc["email"].is_string(),
        "canDelete": orders.is_empty(),
        "defaultAddress": addresses.first().cloned().unwrap_or(Value::Null),
        "addresses": addresses,
        "amountSpent": money(amount_spent(store, id), &cur),
        "numberOfOrders": orders.len().to_string(),
        "lastOrder": orders.last().map_or(Value::Null, |o| reference("Order", id_of(o))),
        "orders": connection("Order", orders.iter().map(|o| reference("Order", id_of(o))).collect()),
        "metafields": metafields,
        "metafield": metafield,
    })
}

pub fn collection(store: &Store, doc: &Value) -> Value {
    let id = id_of(doc);
    let products: Vec<Value> = docs(store, "products")
        .into_iter()
        .filter(|p| {
            p["collections"]
                .as_array()
                .is_some_and(|c| c.iter().any(|cid| cid.as_u64() == Some(id)))
        })
        .map(|p| reference("Product", id_of(p)))
        .collect();
    let html = doc["descriptionHtml"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    json!({
        "__typename": "Collection",
        "id": gid("Collection", id),
        "legacyResourceId": id.to_string(),
        "title": doc["title"],
        "handle": doc["handle"],
        "description": strip_html(&html),
        "descriptionHtml": html,
        "updatedAt": doc.get("updatedAt").cloned().unwrap_or(Value::Null),
        "productsCount": {"__typename": "Count", "count": products.len(), "precision": "EXACT"},
        "products": connection("Product", products),
    })
}

/// The object view for a `__ref`, or null if it no longer exists.
pub fn resolve(store: &Store, kind: &str, id: u64) -> Value {
    match kind {
        "Product" => find(store, "products", id).map_or(Value::Null, |d| product(store, d)),
        "ProductVariant" => variant_owner(store, id)
            .map(|p| {
                let mut p = p.clone();
                normalize_product(&mut p);
                p
            })
            .and_then(|p| {
                let v = p["variants"]
                    .as_array()?
                    .iter()
                    .find(|v| id_of(v) == id)?
                    .clone();
                Some(variant(store, &p, &v))
            })
            .unwrap_or(Value::Null),
        "Order" => find(store, "orders", id).map_or(Value::Null, |d| order(store, d)),
        "Customer" => find(store, "customers", id).map_or(Value::Null, |d| customer(store, d)),
        "Collection" => {
            find(store, "collections", id).map_or(Value::Null, |d| collection(store, d))
        }
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_formats() {
        assert_eq!(parse_cents("12.3"), Some(1230));
        assert_eq!(parse_cents("42"), Some(4200));
        assert_eq!(parse_cents("0.005"), Some(1));
        assert_eq!(parse_cents("abc"), None);
        assert_eq!(money_str(4200), "42.00");
        assert_eq!(decimal_str(4200), "42.0");
        assert_eq!(decimal_str(4250), "42.5");
        assert_eq!(decimal_str(4255), "42.55");
    }

    #[test]
    fn gids_and_html() {
        assert_eq!(parse_gid("gid://shopify/Order/12?x=1"), Some(("Order", 12)));
        assert_eq!(parse_gid("12"), None);
        assert!(is_gid("gid://shopify/Product/abc"));
        assert_eq!(
            strip_html("<p>Soft &amp; warm.</p><p>Hand wash<br>only</p>"),
            "Soft & warm.\nHand wash\nonly"
        );
    }
}
