//! The resource engine shared by every handler: object kinds and ids, rendering stored
//! documents as complete Stripe objects, `expand[]`, cursor-paginated lists, and the
//! search query language.
//!
//! Datasets store Stripe objects keyed by id (`scenarios/<id>/dataset/stripe/<collection>.json`).
//! They may omit fields: rendering fills Stripe's defaults and every nullable field the spec
//! declares, so a curated dataset stays small while responses keep Stripe's full shape.

use std::cmp::Reverse;

use mock_core::world::WorldState;
use serde_json::{Map, Value, json};

use crate::{
    NAME,
    catalog::{self, API_VERSION},
    error::{Res, StripeError},
    params::{Params, as_bool, as_int, scalar_string},
};

pub struct Kind {
    pub object: &'static str,
    pub collection: &'static str,
    pub prefix: &'static str,
    pub len: usize,
    /// List URL, e.g. `/v1/customers`.
    pub url: &'static str,
}

const fn kind(
    object: &'static str,
    collection: &'static str,
    prefix: &'static str,
    len: usize,
    url: &'static str,
) -> Kind {
    Kind {
        object,
        collection,
        prefix,
        len,
        url,
    }
}

pub const KINDS: &[Kind] = &[
    kind("customer", "customers", "cus_", 14, "/v1/customers"),
    kind("charge", "charges", "ch_", 24, "/v1/charges"),
    kind("refund", "refunds", "re_", 24, "/v1/refunds"),
    kind(
        "payment_intent",
        "payment_intents",
        "pi_",
        24,
        "/v1/payment_intents",
    ),
    kind(
        "payment_method",
        "payment_methods",
        "pm_",
        24,
        "/v1/payment_methods",
    ),
    kind("invoice", "invoices", "in_", 24, "/v1/invoices"),
    kind("invoiceitem", "invoiceitems", "ii_", 24, "/v1/invoiceitems"),
    kind(
        "subscription",
        "subscriptions",
        "sub_",
        24,
        "/v1/subscriptions",
    ),
    kind("price", "prices", "price_", 24, "/v1/prices"),
    kind("product", "products", "prod_", 14, "/v1/products"),
    kind("coupon", "coupons", "", 8, "/v1/coupons"),
    kind(
        "promotion_code",
        "promotion_codes",
        "promo_",
        24,
        "/v1/promotion_codes",
    ),
    kind("discount", "discounts", "di_", 24, ""),
    kind(
        "payment_link",
        "payment_links",
        "plink_",
        24,
        "/v1/payment_links",
    ),
    kind(
        "checkout.session",
        "checkout_sessions",
        "cs_test_",
        58,
        "/v1/checkout/sessions",
    ),
    kind("dispute", "disputes", "dp_", 24, "/v1/disputes"),
    kind(
        "balance_transaction",
        "balance_transactions",
        "txn_",
        24,
        "/v1/balance_transactions",
    ),
    kind("payout", "payouts", "po_", 24, "/v1/payouts"),
    kind("event", "events", "evt_", 24, "/v1/events"),
    kind(
        "customer_balance_transaction",
        "customer_balance_transactions",
        "cbtxn_",
        24,
        "",
    ),
    kind(
        "customer_cash_balance_transaction",
        "customer_cash_balance_transactions",
        "ccsbtxn_",
        24,
        "",
    ),
    kind(
        "invoice_payment",
        "invoice_payments",
        "inpay_",
        24,
        "/v1/invoice_payments",
    ),
    kind("credit_note", "credit_notes", "cn_", 24, "/v1/credit_notes"),
    kind(
        "setup_intent",
        "setup_intents",
        "seti_",
        24,
        "/v1/setup_intents",
    ),
    kind(
        "subscription_schedule",
        "subscription_schedules",
        "sub_sched_",
        24,
        "/v1/subscription_schedules",
    ),
    kind("transfer", "transfers", "tr_", 24, "/v1/transfers"),
    kind(
        "application_fee",
        "application_fees",
        "fee_",
        24,
        "/v1/application_fees",
    ),
    kind("account", "accounts", "acct_", 16, "/v1/accounts"),
    kind(
        "billing_portal.configuration",
        "billing_portal_configurations",
        "bpc_",
        24,
        "/v1/billing_portal/configurations",
    ),
    kind(
        "billing_portal.session",
        "billing_portal_sessions",
        "bps_",
        24,
        "/v1/billing_portal/sessions",
    ),
    kind(
        "payment_method_configuration",
        "payment_method_configurations",
        "pmc_",
        24,
        "/v1/payment_method_configurations",
    ),
    kind(
        "payment_method_domain",
        "payment_method_domains",
        "pmd_",
        24,
        "/v1/payment_method_domains",
    ),
    kind(
        "webhook_endpoint",
        "webhook_endpoints",
        "we_",
        24,
        "/v1/webhook_endpoints",
    ),
    kind(
        "test_helpers.test_clock",
        "test_clocks",
        "clock_",
        24,
        "/v1/test_helpers/test_clocks",
    ),
    kind("tax_code", "tax_codes", "txcd_", 8, "/v1/tax_codes"),
];

pub fn kind_of(object: &str) -> &'static Kind {
    KINDS
        .iter()
        .find(|k| k.object == object)
        .unwrap_or_else(|| panic!("unknown object kind {object}"))
}

/// Field names that hold an id of a known object type.
fn field_object(field: &str) -> Option<&'static str> {
    Some(match field {
        "customer" => "customer",
        "charge" | "latest_charge" => "charge",
        "payment_intent" => "payment_intent",
        "invoice" | "latest_invoice" => "invoice",
        "subscription" => "subscription",
        "price" | "default_price" => "price",
        "product" => "product",
        "payment_method" | "default_payment_method" => "payment_method",
        "balance_transaction" | "failure_balance_transaction" => "balance_transaction",
        "promotion_code" => "promotion_code",
        "coupon" => "coupon",
        "payment_link" => "payment_link",
        "setup_intent" | "pending_setup_intent" => "setup_intent",
        "test_clock" => "test_helpers.test_clock",
        "discounts" => "discount",
        "schedule" => "subscription_schedule",
        _ => return None,
    })
}

/// Objects the API renders without `livemode`.
const NO_LIVEMODE: &[&str] = &[
    "balance_transaction",
    "refund",
    "subscription_item",
    "discount",
    "customer_balance_transaction",
];

pub const DEFAULT_ACCOUNT_ID: &str = "acct_1TbNch4mK9xQ2wLp";

/// One request's view of the world.
pub struct Ctx<'a> {
    pub world: &'a mut WorldState,
    pub request_id: Option<String>,
}

impl<'a> Ctx<'a> {
    pub fn new(world: &'a mut WorldState) -> Self {
        Self {
            world,
            request_id: None,
        }
    }

    pub fn now(&self) -> i64 {
        self.world.clock.now_unix()
    }

    pub fn livemode(&self) -> bool {
        self.account()["livemode"].as_bool().unwrap_or(false)
    }

    pub fn docs(&self, collection: &str) -> Vec<(String, Value)> {
        self.world
            .service(NAME)
            .and_then(|s| s.store.collection(collection))
            .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, collection: &str, key: &str) -> Option<Value> {
        self.world
            .service(NAME)
            .and_then(|s| s.store.collection(collection))
            .and_then(|c| c.get(key))
            .cloned()
    }

    pub fn put(&mut self, collection: &str, key: &str, doc: Value) {
        self.world
            .service_mut(NAME)
            .store
            .collection_mut(collection)
            .insert(key.to_owned(), doc);
    }

    pub fn delete(&mut self, collection: &str, key: &str) -> Option<Value> {
        self.world
            .service_mut(NAME)
            .store
            .collection_mut(collection)
            .remove(key)
    }

    pub fn id(&mut self, object: &str) -> String {
        let k = kind_of(object);
        self.raw_id(k.object, k.prefix, k.len)
    }

    pub fn raw_id(&mut self, kind: &str, prefix: &str, len: usize) -> String {
        self.world
            .service_mut(NAME)
            .ids
            .next_prefixed(kind, prefix, len)
    }

    /// The account this sandbox session targets (dataset `account.json`, first document).
    pub fn account(&self) -> Value {
        let stored = self
            .docs("account")
            .into_iter()
            .next()
            .map(|(_, v)| v)
            .unwrap_or_else(|| json!({}));
        let mut account = json!({
            "id": DEFAULT_ACCOUNT_ID,
            "object": "account",
            "business_profile": {
                "mcc": "5734",
                "name": "Tool Bench Sandbox",
                "support_address": null,
                "support_email": null,
                "support_phone": null,
                "url": null
            },
            "business_type": "company",
            "capabilities": {"card_payments": "active", "transfers": "active"},
            "charges_enabled": true,
            "country": "US",
            "created": 1_735_689_600,
            "default_currency": "usd",
            "details_submitted": true,
            "email": null,
            "livemode": false,
            "metadata": {},
            "payouts_enabled": true,
            "settings": {
                "dashboard": {"display_name": "Tool Bench Sandbox", "timezone": "Etc/UTC"},
                "payouts": {"schedule": {"delay_days": 2, "interval": "daily"}}
            },
            "type": "standard"
        });
        merge(&mut account, &stored);
        account
    }

    pub fn account_name(&self) -> String {
        let account = self.account();
        account["settings"]["dashboard"]["display_name"]
            .as_str()
            .or_else(|| account["business_profile"]["name"].as_str())
            .unwrap_or("Stripe account")
            .to_owned()
    }

    /// The request id of this write (`req_…`), minted once.
    pub fn request(&mut self) -> String {
        if self.request_id.is_none() {
            let id = self.raw_id("request", "req_", 14);
            self.request_id = Some(id);
        }
        self.request_id.clone().expect("set above")
    }

    /// Records an event for a mutation, as Stripe does for every state change.
    pub fn emit(&mut self, event_type: &str, object: &str, doc: &Value, previous: Option<Value>) {
        let rendered = self.render(object, doc);
        let request = self.request();
        let id = self.id("event");
        let mut data = json!({ "object": rendered });
        if let Some(prev) = previous.filter(|p| p.as_object().is_some_and(|m| !m.is_empty())) {
            data["previous_attributes"] = prev;
        }
        let event = json!({
            "id": id,
            "object": "event",
            "api_version": API_VERSION,
            "created": self.now(),
            "data": data,
            "livemode": self.livemode(),
            "pending_webhooks": 0,
            "request": {"id": request, "idempotency_key": null},
            "type": event_type
        });
        self.put("events", &id, event);
    }

    /// Finds an object by id: by prefix first, then (for custom ids) in every collection.
    pub fn find_any(&self, id: &str, hint: Option<&str>) -> Option<(&'static str, Value)> {
        if let Some(object) = hint
            && let Some(doc) = self.lookup(object, id)
        {
            return Some((kind_of(object).object, doc));
        }
        let mut by_prefix: Vec<&Kind> = KINDS
            .iter()
            .filter(|k| !k.prefix.is_empty() && id.starts_with(k.prefix))
            .collect();
        by_prefix.sort_by_key(|k| Reverse(k.prefix.len()));
        for k in by_prefix {
            if let Some(doc) = self.lookup(k.object, id) {
                return Some((k.object, doc));
            }
        }
        KINDS
            .iter()
            .find_map(|k| self.lookup(k.object, id).map(|doc| (k.object, doc)))
    }

    /// A document by object type and id. Discounts are keyed by `<parent>:<coupon>`, so they
    /// are found by their `id` field.
    pub fn lookup(&self, object: &str, id: &str) -> Option<Value> {
        let k = kind_of(object);
        if object == "discount" {
            return self
                .docs(k.collection)
                .into_iter()
                .map(|(_, v)| v)
                .find(|d| d["id"] == id);
        }
        self.get(k.collection, id)
    }

    /// Like [`Ctx::lookup`] but answers Stripe's 404 (`param` names the parameter).
    pub fn require(&self, object: &str, id: &str, param: &str) -> Res<Value> {
        let name = object_label(object);
        let doc = self
            .lookup(object, id)
            .ok_or_else(|| StripeError::missing(&name, id, param))?;
        if doc["deleted"] == true && object != "customer" {
            return Err(StripeError::missing(&name, id, param));
        }
        Ok(doc)
    }

    /// The stored document as a complete Stripe object.
    pub fn render(&self, object: &str, doc: &Value) -> Value {
        if doc["deleted"] == true {
            return sorted(doc.clone());
        }
        let mut out = doc.clone();
        if !out.is_object() {
            return out;
        }
        {
            let map = out.as_object_mut().expect("object");
            fill(map, "object", json!(object));
            if !NO_LIVEMODE.contains(&object) {
                fill(map, "livemode", json!(self.livemode()));
            }
        }
        self.defaults(object, &mut out);
        let map = out.as_object_mut().expect("object");
        for field in catalog::nullable_fields(object) {
            if !OMITTED.iter().any(|(o, f)| *o == object && *f == field) {
                fill(map, field, Value::Null);
            }
        }
        sorted(out)
    }

    /// Renders and applies `expand[]` paths.
    pub fn present(&self, object: &str, doc: &Value, expand: &[String]) -> Res<Value> {
        let mut out = self.render(object, doc);
        for path in expand {
            let segments: Vec<&str> = path.split('.').collect();
            self.expand(&mut out, &segments)?;
        }
        Ok(out)
    }

    fn expand(&self, value: &mut Value, path: &[&str]) -> Res<()> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(());
        };
        if let Value::Array(items) = value {
            for item in items {
                self.expand(item, path)?;
            }
            return Ok(());
        }
        let object = value["object"].as_str().unwrap_or_default().to_owned();
        let Some(map) = value.as_object_mut() else {
            return Err(cannot_expand(first));
        };
        if let Some(virt) = self.virtual_field(&object, map, first) {
            map.insert((*first).to_owned(), virt);
        } else {
            let expandable = catalog::expandable_fields(&object);
            let known = expandable.contains(first) || field_object(first).is_some();
            match map.get(*first).cloned() {
                Some(Value::String(id)) if known => {
                    if let Some((obj, doc)) = self.find_any(&id, field_object(first)) {
                        map.insert((*first).to_owned(), self.render(obj, &doc));
                    }
                }
                Some(Value::Array(items)) if known => {
                    let expanded: Vec<Value> = items
                        .into_iter()
                        .map(|item| match &item {
                            Value::String(id) => self
                                .find_any(id, field_object(first))
                                .map(|(obj, doc)| self.render(obj, &doc))
                                .unwrap_or(item),
                            _ => item,
                        })
                        .collect();
                    map.insert((*first).to_owned(), Value::Array(expanded));
                }
                Some(Value::Object(_)) | Some(Value::Null) if known => {}
                _ => return Err(cannot_expand(first)),
            }
        }
        if !rest.is_empty()
            && let Some(child) = map.get_mut(*first)
        {
            if child.is_null() {
                return Ok(());
            }
            self.expand(child, rest)?;
        }
        Ok(())
    }

    /// Expand-only fields that are not stored: lists hanging off an object.
    fn virtual_field(&self, object: &str, map: &Map<String, Value>, field: &str) -> Option<Value> {
        let id = map.get("id")?.as_str()?.to_owned();
        let docs = |collection: &str, key: &str| -> Vec<Value> {
            let mut items: Vec<Value> = self
                .docs(collection)
                .into_iter()
                .map(|(_, v)| v)
                .filter(|d| d[key] == id.as_str() && d["deleted"] != true)
                .collect();
            sort_newest_first(&mut items);
            items
        };
        let list = |object: &str, items: Vec<Value>, url: String| {
            let data: Vec<Value> = items.iter().map(|d| self.render(object, d)).collect();
            json!({
                "object": "list",
                "data": data,
                "has_more": false,
                "total_count": data.len(),
                "url": url
            })
        };
        match (object, field) {
            ("charge", "refunds") => Some(list(
                "refund",
                docs("refunds", "charge"),
                format!("/v1/charges/{id}/refunds"),
            )),
            ("customer", "subscriptions") => Some(list(
                "subscription",
                docs("subscriptions", "customer")
                    .into_iter()
                    .filter(|s| s["status"] != "canceled")
                    .collect(),
                format!("/v1/customers/{id}/subscriptions"),
            )),
            ("customer", "tax_ids") => Some(list(
                "tax_id",
                Vec::new(),
                format!("/v1/customers/{id}/tax_ids"),
            )),
            ("customer", "sources") => Some(list(
                "source",
                Vec::new(),
                format!("/v1/customers/{id}/sources"),
            )),
            ("invoice", "payments") => Some(list(
                "invoice_payment",
                docs("invoice_payments", "invoice"),
                format!("/v1/invoices/{id}/payments"),
            )),
            ("checkout.session", "line_items") | ("payment_link", "line_items") => {
                let items = self
                    .lookup(object, &id)
                    .and_then(|d| d["_line_items"].as_array().cloned())
                    .unwrap_or_default();
                let base = if object == "payment_link" {
                    "payment_links"
                } else {
                    "checkout/sessions"
                };
                Some(json!({
                    "object": "list",
                    "data": items,
                    "has_more": false,
                    "url": format!("/v1/{base}/{id}/line_items")
                }))
            }
            _ => None,
        }
    }

    /// Defaults for fields the dataset may leave out, and derived amounts.
    fn defaults(&self, object: &str, out: &mut Value) {
        let created = out["created"].as_i64().unwrap_or_else(|| self.now());
        let id = out["id"].as_str().unwrap_or_default().to_owned();
        let map = out.as_object_mut().expect("object");
        if !matches!(
            object,
            "discount" | "balance" | "cash_balance" | "line_item" | "item"
        ) {
            fill(map, "created", json!(created));
        }
        match object {
            "customer" => {
                fill(map, "balance", json!(0));
                fill(map, "delinquent", json!(false));
                fill(
                    map,
                    "invoice_settings",
                    json!({"custom_fields": null, "default_payment_method": null, "footer": null, "rendering_options": null}),
                );
                fill(map, "metadata", json!({}));
                fill(map, "next_invoice_sequence", json!(1));
                fill(map, "preferred_locales", json!([]));
                fill(map, "tax_exempt", json!("none"));
            }
            "charge" => {
                let amount = map["amount"].as_i64().unwrap_or(0);
                fill(map, "status", json!("succeeded"));
                let succeeded = map["status"] == "succeeded";
                fill(map, "captured", json!(succeeded));
                let captured = map["captured"] == true;
                fill(
                    map,
                    "amount_captured",
                    json!(if captured { amount } else { 0 }),
                );
                fill(map, "amount_refunded", json!(0));
                let refunded = map["amount_refunded"].as_i64().unwrap_or(0);
                fill(map, "refunded", json!(amount > 0 && refunded >= amount));
                fill(map, "paid", json!(succeeded));
                fill(map, "disputed", json!(false));
                fill(map, "currency", json!("usd"));
                fill(map, "metadata", json!({}));
                fill(map, "billing_details", billing_details());
                fill(map, "fraud_details", json!({}));
                fill(
                    map,
                    "outcome",
                    if succeeded {
                        json!({"advice_code": null, "network_advice_code": null, "network_decline_code": null, "network_status": "approved_by_network", "reason": null, "risk_level": "normal", "risk_score": 32, "seller_message": "Payment complete.", "type": "authorized"})
                    } else {
                        json!({"advice_code": null, "network_advice_code": null, "network_decline_code": "05", "network_status": "declined_by_network", "reason": "generic_decline", "risk_level": "normal", "risk_score": 40, "seller_message": "The bank did not return any further details with this decline.", "type": "issuer_declined"})
                    },
                );
                fill(
                    map,
                    "receipt_url",
                    json!(format!("https://pay.stripe.com/receipts/payment/{id}")),
                );
            }
            "refund" => {
                fill(map, "status", json!("succeeded"));
                fill(map, "metadata", json!({}));
                fill(map, "currency", json!("usd"));
                fill(
                    map,
                    "destination_details",
                    json!({"card": {"reference_status": "pending", "reference_type": "acquirer_reference_number", "type": "refund"}, "type": "card"}),
                );
            }
            "payment_intent" => {
                let amount = map["amount"].as_i64().unwrap_or(0);
                fill(map, "status", json!("succeeded"));
                let succeeded = map["status"] == "succeeded";
                fill(
                    map,
                    "amount_received",
                    json!(if succeeded { amount } else { 0 }),
                );
                fill(map, "amount_capturable", json!(0));
                fill(map, "amount_details", json!({"tip": {}}));
                fill(
                    map,
                    "automatic_payment_methods",
                    json!({"allow_redirects": "always", "enabled": true}),
                );
                fill(map, "capture_method", json!("automatic_async"));
                fill(map, "confirmation_method", json!("automatic"));
                fill(map, "currency", json!("usd"));
                fill(map, "metadata", json!({}));
                fill(map, "payment_method_types", json!(["card"]));
                fill(
                    map,
                    "client_secret",
                    json!(format!("{id}_secret_{}", short_hash(&id))),
                );
            }
            "payment_method" => {
                fill(map, "type", json!("card"));
                fill(map, "allow_redisplay", json!("unspecified"));
                fill(map, "billing_details", billing_details());
                fill(map, "metadata", json!({}));
            }
            "invoice" => self.invoice_defaults(map, &id, created),
            "invoiceitem" => {
                fill(map, "date", json!(created));
                fill(map, "currency", json!("usd"));
                fill(map, "discountable", json!(true));
                fill(map, "discounts", json!([]));
                fill(map, "metadata", json!({}));
                fill(map, "period", json!({"end": created, "start": created}));
                fill(map, "proration", json!(false));
                fill(map, "quantity", json!(1));
                fill(map, "tax_rates", json!([]));
            }
            "subscription" => self.subscription_defaults(map, &id, created),
            "price" => {
                fill(map, "active", json!(true));
                fill(map, "billing_scheme", json!("per_unit"));
                fill(map, "currency", json!("usd"));
                fill(map, "metadata", json!({}));
                let recurring = map.get("recurring").is_some_and(|r| r.is_object());
                fill(
                    map,
                    "type",
                    json!(if recurring { "recurring" } else { "one_time" }),
                );
                if let Some(Value::Object(r)) = map.get_mut("recurring") {
                    fill(r, "interval_count", json!(1));
                    fill(r, "meter", Value::Null);
                    fill(r, "trial_period_days", Value::Null);
                    fill(r, "usage_type", json!("licensed"));
                }
                if let Some(amount) = map.get("unit_amount").and_then(Value::as_i64) {
                    fill(map, "unit_amount_decimal", json!(amount.to_string()));
                }
                fill(map, "tax_behavior", json!("unspecified"));
            }
            "product" => {
                fill(map, "active", json!(true));
                fill(map, "images", json!([]));
                fill(map, "marketing_features", json!([]));
                fill(map, "metadata", json!({}));
                fill(map, "updated", json!(created));
            }
            "coupon" => {
                fill(map, "duration", json!("once"));
                fill(map, "metadata", json!({}));
                fill(map, "times_redeemed", json!(0));
                fill(map, "valid", json!(true));
            }
            "promotion_code" => {
                fill(map, "active", json!(true));
                fill(map, "metadata", json!({}));
                fill(
                    map,
                    "restrictions",
                    json!({"first_time_transaction": false, "minimum_amount": null, "minimum_amount_currency": null}),
                );
                fill(map, "times_redeemed", json!(0));
            }
            "discount" => {
                fill(map, "start", json!(created));
                map.remove("created");
                if let Some(coupon) = map["source"]["coupon"].as_str().map(str::to_owned)
                    && let Some(doc) = self.lookup("coupon", &coupon)
                {
                    map.insert(
                        "source".into(),
                        json!({"coupon": self.render("coupon", &doc), "type": "coupon"}),
                    );
                }
            }
            "dispute" => {
                fill(map, "balance_transactions", json!([]));
                fill(map, "currency", json!("usd"));
                fill(map, "evidence", dispute_evidence());
                fill(
                    map,
                    "evidence_details",
                    json!({"due_by": created + 7 * 86_400, "enhanced_eligibility": {}, "has_evidence": false, "past_due": false, "submission_count": 0}),
                );
                fill(map, "is_charge_refundable", json!(false));
                fill(map, "metadata", json!({}));
                fill(map, "reason", json!("general"));
                fill(map, "status", json!("needs_response"));
            }
            "balance_transaction" => {
                let amount = map["amount"].as_i64().unwrap_or(0);
                let fee = map["fee"].as_i64().unwrap_or(0);
                fill(map, "available_on", json!(created + 2 * 86_400));
                fill(map, "currency", json!("usd"));
                fill(map, "fee", json!(fee));
                fill(map, "fee_details", json!([]));
                fill(map, "net", json!(amount - fee));
                fill(map, "status", json!("available"));
                fill(map, "type", json!("charge"));
                let category = match map["type"].as_str().unwrap_or("charge") {
                    "payment" => "charge",
                    other => other,
                }
                .to_owned();
                fill(map, "reporting_category", json!(category));
            }
            "payout" => {
                fill(map, "arrival_date", json!(created + 2 * 86_400));
                fill(map, "automatic", json!(true));
                fill(map, "currency", json!("usd"));
                fill(map, "metadata", json!({}));
                fill(map, "method", json!("standard"));
                fill(map, "reconciliation_status", json!("completed"));
                fill(map, "source_type", json!("card"));
                fill(map, "status", json!("paid"));
                fill(map, "type", json!("bank_account"));
            }
            "payment_link" => {
                map.remove("_line_items");
                fill(map, "active", json!(true));
                fill(
                    map,
                    "after_completion",
                    json!({"hosted_confirmation": {"custom_message": null}, "type": "hosted_confirmation"}),
                );
                fill(map, "allow_promotion_codes", json!(false));
                fill(
                    map,
                    "automatic_tax",
                    json!({"enabled": false, "liability": null}),
                );
                fill(map, "billing_address_collection", json!("auto"));
                fill(map, "currency", json!("usd"));
                fill(map, "custom_fields", json!([]));
                fill(
                    map,
                    "custom_text",
                    json!({"after_submit": null, "shipping_address": null, "submit": null, "terms_of_service_acceptance": null}),
                );
                fill(map, "customer_creation", json!("if_required"));
                fill(map, "metadata", json!({}));
                fill(map, "payment_method_collection", json!("always"));
                fill(map, "phone_number_collection", json!({"enabled": false}));
                fill(map, "shipping_options", json!([]));
                fill(map, "submit_type", json!("auto"));
                fill(
                    map,
                    "tax_id_collection",
                    json!({"enabled": false, "required": "never"}),
                );
                fill(
                    map,
                    "url",
                    json!(format!("https://buy.stripe.com/test_{}", short_hash(&id))),
                );
            }
            "checkout.session" => {
                map.remove("_line_items");
                fill(map, "expires_at", json!(created + 86_400));
                fill(map, "currency", json!("usd"));
                fill(map, "custom_fields", json!([]));
                fill(map, "metadata", json!({}));
                fill(map, "mode", json!("payment"));
                fill(map, "payment_method_types", json!(["card"]));
                fill(map, "payment_status", json!("unpaid"));
                fill(map, "status", json!("open"));
                fill(map, "ui_mode", json!("hosted"));
                let open = map["status"] == "open";
                fill(
                    map,
                    "url",
                    if open {
                        json!(format!("https://checkout.stripe.com/c/pay/{id}"))
                    } else {
                        Value::Null
                    },
                );
            }
            "event" => {
                fill(map, "api_version", json!(API_VERSION));
                fill(map, "pending_webhooks", json!(0));
            }
            _ => {
                if catalog::objects()
                    .get(object)
                    .is_some_and(|o| o["nullable"].as_array().is_some())
                {
                    fill(map, "metadata", json!({}));
                }
            }
        }
    }

    fn invoice_defaults(&self, map: &mut Map<String, Value>, id: &str, created: i64) {
        fill(map, "currency", json!("usd"));
        fill(map, "status", json!("draft"));
        let account = self.account();
        fill(map, "account_country", account["country"].clone());
        fill(map, "account_name", json!(self.account_name()));
        let currency = map["currency"].clone();
        let customer = map["customer"].clone();
        let lines: Vec<Value> = map["lines"]["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut line| {
                if let Some(l) = line.as_object_mut() {
                    let amount = l["amount"].as_i64().unwrap_or(0);
                    fill(l, "object", json!("line_item"));
                    fill(l, "currency", currency.clone());
                    fill(l, "description", Value::Null);
                    fill(l, "discount_amounts", json!([]));
                    fill(l, "discountable", json!(true));
                    fill(l, "discounts", json!([]));
                    fill(l, "invoice", json!(id));
                    fill(l, "livemode", json!(self.livemode()));
                    fill(l, "metadata", json!({}));
                    fill(l, "parent", Value::Null);
                    fill(l, "period", json!({"end": created, "start": created}));
                    fill(l, "pretax_credit_amounts", json!([]));
                    fill(l, "pricing", Value::Null);
                    fill(l, "quantity", json!(1));
                    fill(l, "subtotal", json!(amount));
                    fill(l, "taxes", json!([]));
                }
                sorted(line)
            })
            .collect();
        let subtotal: i64 = lines.iter().filter_map(|l| l["amount"].as_i64()).sum();
        let count = lines.len();
        map.insert(
            "lines".into(),
            json!({
                "object": "list",
                "data": lines,
                "has_more": false,
                "total_count": count,
                "url": format!("/v1/invoices/{id}/lines")
            }),
        );
        fill(map, "subtotal", json!(subtotal));
        fill(map, "subtotal_excluding_tax", json!(subtotal));
        let total = map["subtotal"].as_i64().unwrap_or(subtotal);
        fill(map, "total", json!(total));
        fill(map, "total_excluding_tax", json!(total));
        let total = map["total"].as_i64().unwrap_or(total);
        let status = map["status"].as_str().unwrap_or("draft").to_owned();
        fill(map, "amount_due", json!(total));
        let due = map["amount_due"].as_i64().unwrap_or(total);
        fill(
            map,
            "amount_paid",
            json!(if status == "paid" { due } else { 0 }),
        );
        let paid = map["amount_paid"].as_i64().unwrap_or(0);
        fill(
            map,
            "amount_remaining",
            json!(if status == "void" { 0 } else { due - paid }),
        );
        fill(map, "amount_overpaid", json!(0));
        fill(map, "amount_paid_off_stripe", json!(0));
        fill(map, "amount_shipping", json!(0));
        fill(
            map,
            "attempt_count",
            json!(if status == "paid" { 1 } else { 0 }),
        );
        fill(map, "attempted", json!(status == "paid"));
        fill(
            map,
            "auto_advance",
            json!(status == "draft" || status == "open"),
        );
        fill(
            map,
            "automatic_tax",
            json!({"disabled_reason": null, "enabled": false, "liability": null, "provider": null, "status": null}),
        );
        fill(map, "collection_method", json!("charge_automatically"));
        fill(map, "customer", customer);
        fill(map, "default_tax_rates", json!([]));
        fill(map, "discounts", json!([]));
        fill(map, "issuer", json!({"type": "self"}));
        fill(map, "metadata", json!({}));
        fill(
            map,
            "payment_settings",
            json!({"default_mandate": null, "payment_method_options": null, "payment_method_types": null}),
        );
        fill(map, "period_end", json!(created));
        fill(map, "period_start", json!(created));
        fill(map, "post_payment_credit_notes_amount", json!(0));
        fill(map, "pre_payment_credit_notes_amount", json!(0));
        fill(map, "starting_balance", json!(0));
        fill(
            map,
            "ending_balance",
            json!(if status == "draft" {
                Value::Null
            } else {
                json!(0)
            }),
        );
        fill(
            map,
            "status_transitions",
            json!({"finalized_at": null, "marked_uncollectible_at": null, "paid_at": null, "voided_at": null}),
        );
        fill(map, "total_discount_amounts", json!([]));
        fill(map, "total_pretax_credit_amounts", json!([]));
        fill(map, "total_taxes", json!([]));
        if status != "draft" {
            let hosted = format!("https://invoice.stripe.com/i/acct_test/{}", short_hash(id));
            fill(map, "hosted_invoice_url", json!(hosted));
            fill(map, "invoice_pdf", json!(format!("{hosted}/pdf")));
        }
    }

    fn subscription_defaults(&self, map: &mut Map<String, Value>, id: &str, created: i64) {
        fill(map, "start_date", json!(created));
        let start = map["start_date"].as_i64().unwrap_or(created);
        fill(map, "billing_cycle_anchor", json!(start));
        let anchor = map["billing_cycle_anchor"].as_i64().unwrap_or(start);
        fill(map, "status", json!("active"));
        fill(
            map,
            "automatic_tax",
            json!({"disabled_reason": null, "enabled": false, "liability": null}),
        );
        fill(
            map,
            "billing_mode",
            json!({"flexible": null, "type": "classic"}),
        );
        fill(map, "billing_schedules", json!([]));
        fill(map, "cancel_at_period_end", json!(false));
        fill(
            map,
            "cancellation_details",
            json!({"comment": null, "feedback": null, "reason": null}),
        );
        fill(map, "collection_method", json!("charge_automatically"));
        fill(map, "currency", json!("usd"));
        fill(map, "default_tax_rates", json!([]));
        fill(map, "discounts", json!([]));
        fill(
            map,
            "invoice_settings",
            json!({"account_tax_ids": null, "issuer": {"type": "self"}}),
        );
        fill(map, "metadata", json!({}));
        fill(
            map,
            "payment_settings",
            json!({"payment_method_options": null, "payment_method_types": null, "save_default_payment_method": "off"}),
        );
        fill(
            map,
            "trial_settings",
            json!({"end_behavior": {"missing_payment_method": "create_invoice"}}),
        );
        let items: Vec<Value> = map["items"]["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|item| self.render_item(item, id, created, anchor))
            .collect();
        let count = items.len();
        map.insert(
            "items".into(),
            json!({
                "object": "list",
                "data": items,
                "has_more": false,
                "total_count": count,
                "url": format!("/v1/subscription_items?subscription={id}")
            }),
        );
    }

    pub fn render_item(&self, item: Value, sub: &str, created: i64, anchor: i64) -> Value {
        let mut item = item;
        let Some(map) = item.as_object_mut() else {
            return item;
        };
        fill(map, "object", json!("subscription_item"));
        fill(map, "billing_thresholds", Value::Null);
        fill(map, "created", json!(created));
        fill(map, "discounts", json!([]));
        fill(map, "metadata", json!({}));
        fill(map, "quantity", json!(1));
        fill(map, "subscription", json!(sub));
        fill(map, "tax_rates", json!([]));
        let price = match map.get("price") {
            Some(Value::String(pid)) => self
                .lookup("price", pid)
                .map(|p| self.render("price", &p))
                .unwrap_or_else(|| json!(pid)),
            Some(other) => other.clone(),
            None => Value::Null,
        };
        let (interval, count) = price_interval(&price);
        fill(map, "current_period_start", json!(anchor));
        let start = map["current_period_start"].as_i64().unwrap_or(anchor);
        fill(
            map,
            "current_period_end",
            json!(add_interval(start, &interval, count)),
        );
        map.insert("price".into(), price);
        sorted(item)
    }

    /// Rendered list of stored documents: newest first, cursor-paginated, `expand[]` applied.
    pub fn list(
        &self,
        object: &str,
        url: &str,
        params: &Params,
        filter: &dyn Fn(&Value) -> bool,
    ) -> Res<Value> {
        let k = kind_of(object);
        let mut docs: Vec<Value> = self
            .docs(k.collection)
            .into_iter()
            .map(|(_, v)| v)
            .filter(|d| d["deleted"] != true)
            .collect();
        if let Some(created) = params.raw("created") {
            docs.retain(|d| range_match(&d["created"], created));
        }
        docs.retain(|d| filter(d));
        sort_newest_first(&mut docs);
        let rendered: Vec<Value> = docs.iter().map(|d| self.render(object, d)).collect();
        self.page(object, url, rendered, params)
    }

    /// Cursor pagination over already-rendered objects (newest first).
    pub fn page(&self, object: &str, url: &str, items: Vec<Value>, params: &Params) -> Res<Value> {
        let limit = list_limit(params)?;
        let label = object_label(object);
        let position = |cursor: &str, param: &str| -> Res<usize> {
            items
                .iter()
                .position(|i| i["id"] == cursor)
                .ok_or_else(|| StripeError::missing(&label, cursor, param).status(400))
        };
        let (slice, has_more): (Vec<Value>, bool) =
            if let Some(after) = params.str("starting_after").filter(|s| !s.is_empty()) {
                let start = position(&after, "starting_after")? + 1;
                let rest: Vec<Value> = items[start..].to_vec();
                let more = rest.len() > limit;
                (rest.into_iter().take(limit).collect(), more)
            } else if let Some(before) = params.str("ending_before").filter(|s| !s.is_empty()) {
                let end = position(&before, "ending_before")?;
                let from = end.saturating_sub(limit);
                (items[from..end].to_vec(), from > 0)
            } else {
                let more = items.len() > limit;
                (items.into_iter().take(limit).collect(), more)
            };
        let expand = data_expand(params)?;
        let mut data = Vec::with_capacity(slice.len());
        for mut item in slice {
            for path in &expand {
                let segments: Vec<&str> = path.split('.').collect();
                self.expand(&mut item, &segments)?;
            }
            data.push(item);
        }
        Ok(json!({
            "object": "list",
            "data": data,
            "has_more": has_more,
            "url": url
        }))
    }

    /// `GET /v1/<resource>/search` with Stripe's query language.
    pub fn search(
        &self,
        object: &str,
        resource: &str,
        params: &Params,
        fields: &[&str],
    ) -> Res<Value> {
        let query = params.req_str("query")?;
        let clauses = parse_query(&query, resource, fields)?;
        let k = kind_of(object);
        let mut docs: Vec<Value> = self
            .docs(k.collection)
            .into_iter()
            .map(|(_, v)| v)
            .filter(|d| d["deleted"] != true)
            .map(|d| self.render(object, &d))
            .filter(|d| clauses.matches(d))
            .collect();
        sort_newest_first(&mut docs);
        let limit = list_limit(params)?;
        let offset = match params.str("page").filter(|p| !p.is_empty()) {
            None => 0,
            Some(page) => decode_page(&page).ok_or_else(|| {
                StripeError::invalid(format!("Invalid page: {page}")).param("page")
            })?,
        };
        let total = docs.len();
        let slice: Vec<Value> = docs.into_iter().skip(offset).take(limit).collect();
        let has_more = offset + slice.len() < total;
        let expand = data_expand(params)?;
        let mut data = Vec::with_capacity(slice.len());
        for mut item in slice {
            for path in &expand {
                let segments: Vec<&str> = path.split('.').collect();
                self.expand(&mut item, &segments)?;
            }
            data.push(item);
        }
        let mut out = json!({
            "object": "search_result",
            "data": data,
            "has_more": has_more,
            "next_page": if has_more { json!(encode_page(offset + limit)) } else { Value::Null },
            "url": format!("/v1/{resource}/search")
        });
        if params.expand().iter().any(|e| e == "total_count") {
            out["total_count"] = json!(total);
        }
        Ok(out)
    }
}

/// Fields that are nullable in the spec but only appear when expanded.
const OMITTED: &[(&str, &str)] = &[("charge", "refunds")];

pub fn object_label(object: &str) -> String {
    match object {
        "payment_method" => "PaymentMethod".to_owned(),
        "payment_intent" | "checkout.session" | "invoiceitem" | "setup_intent" => object.to_owned(),
        other => other.replace('_', " "),
    }
}

fn cannot_expand(field: &str) -> StripeError {
    StripeError::invalid(format!("This property cannot be expanded ({field})."))
}

/// `expand[]` on a list: every path must start with `data.`.
fn data_expand(params: &Params) -> Res<Vec<String>> {
    let mut out = Vec::new();
    for path in params.expand() {
        if path == "total_count" {
            continue;
        }
        match path.strip_prefix("data.") {
            Some(rest) => out.push(rest.to_owned()),
            None => {
                return Err(StripeError::invalid(format!(
                    "This property cannot be expanded ({path}). You may want to try expanding 'data.{path}' instead."
                )));
            }
        }
    }
    Ok(out)
}

fn list_limit(params: &Params) -> Res<usize> {
    Ok(params.int("limit")?.unwrap_or(10).clamp(1, 100) as usize)
}

fn encode_page(offset: usize) -> String {
    let hex: String = format!("{offset:08x}").chars().rev().collect();
    format!("page_{hex}")
}

fn decode_page(page: &str) -> Option<usize> {
    let hex: String = page.strip_prefix("page_")?.chars().rev().collect();
    usize::from_str_radix(&hex, 16).ok()
}

/// Newest first; ties keep the newer insertion first (later dataset rows, later creates).
pub fn sort_newest_first(items: &mut [Value]) {
    items.reverse();
    items.sort_by_key(|d| Reverse(d["created"].as_i64().unwrap_or(0)));
}

/// `created=1700000000` or `created[gte]=…` style filters.
pub fn range_match(value: &Value, filter: &Value) -> bool {
    let Some(v) = value.as_i64() else {
        return false;
    };
    match filter {
        Value::Object(ops) => ops.iter().all(|(op, bound)| {
            let Some(b) = as_int(bound) else { return true };
            match op.as_str() {
                "gt" => v > b,
                "gte" => v >= b,
                "lt" => v < b,
                "lte" => v <= b,
                _ => true,
            }
        }),
        other => as_int(other).is_none_or(|b| v == b),
    }
}

pub fn fill(map: &mut Map<String, Value>, key: &str, value: Value) {
    if !map.contains_key(key) {
        map.insert(key.to_owned(), value);
    }
}

/// Recursively merges `patch` into `base` (objects merge, everything else replaces).
pub fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(existing) if existing.is_object() && v.is_object() => merge(existing, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// `id` and `object` first, then the remaining keys in alphabetical order, recursively.
pub fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> =
                map.into_iter().map(|(k, v)| (k, sorted(v))).collect();
            entries.sort_by(|(a, _), (b, _)| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
            Value::Object(entries.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn rank(key: &str) -> u8 {
    match key {
        "id" => 0,
        "object" => 1,
        _ => 2,
    }
}

pub fn short_hash(text: &str) -> String {
    // FNV-1a, rendered base62: a stable decoration for URLs and secrets.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    const B62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = String::new();
    for _ in 0..14 {
        out.push(B62[(h % 62) as usize] as char);
        h /= 62;
        if h == 0 {
            h = 0x9e37_79b9_7f4a_7c15;
        }
    }
    out
}

fn billing_details() -> Value {
    json!({
        "address": {"city": null, "country": null, "line1": null, "line2": null, "postal_code": null, "state": null},
        "email": null,
        "name": null,
        "phone": null,
        "tax_id": null
    })
}

fn dispute_evidence() -> Value {
    let fields = [
        "access_activity_log",
        "billing_address",
        "cancellation_policy",
        "cancellation_policy_disclosure",
        "cancellation_rebuttal",
        "customer_communication",
        "customer_email_address",
        "customer_name",
        "customer_purchase_ip",
        "customer_signature",
        "duplicate_charge_documentation",
        "duplicate_charge_explanation",
        "duplicate_charge_id",
        "product_description",
        "receipt",
        "refund_policy",
        "refund_policy_disclosure",
        "refund_refusal_explanation",
        "service_date",
        "service_documentation",
        "shipping_address",
        "shipping_carrier",
        "shipping_date",
        "shipping_documentation",
        "shipping_tracking_number",
        "uncategorized_file",
        "uncategorized_text",
    ];
    let mut map: Map<String, Value> = fields
        .iter()
        .map(|f| ((*f).to_owned(), Value::Null))
        .collect();
    map.insert("enhanced_evidence".into(), json!({}));
    Value::Object(map)
}

/// `(interval, interval_count)` of a rendered price, `("month", 1)` when unknown.
pub fn price_interval(price: &Value) -> (String, i64) {
    let interval = price["recurring"]["interval"]
        .as_str()
        .unwrap_or("month")
        .to_owned();
    let count = price["recurring"]["interval_count"].as_i64().unwrap_or(1);
    (interval, count)
}

/// `start` advanced by `count` intervals, calendar-aware for months and years.
pub fn add_interval(start: i64, interval: &str, count: i64) -> i64 {
    match interval {
        "day" => start + count * 86_400,
        "week" => start + count * 7 * 86_400,
        "year" => add_months(start, 12 * count),
        _ => add_months(start, count),
    }
}

fn add_months(start: i64, months: i64) -> i64 {
    let Ok(dt) = time::OffsetDateTime::from_unix_timestamp(start) else {
        return start + months * 30 * 86_400;
    };
    let total = i64::from(dt.year()) * 12 + i64::from(u8::from(dt.month())) - 1 + months;
    let year = (total.div_euclid(12)) as i32;
    let month = time::Month::try_from((total.rem_euclid(12) + 1) as u8).expect("month 1-12");
    let last_day = time::util::days_in_month(month, year);
    let day = dt.day().min(last_day);
    let date = time::Date::from_calendar_date(year, month, day).expect("valid date");
    dt.replace_date(date).unix_timestamp()
}

// --- Search query language -----------------------------------------------------------

#[derive(Debug)]
struct Clause {
    negate: bool,
    field: Vec<String>,
    op: String,
    value: Value,
}

#[derive(Debug)]
struct Query {
    any: bool,
    clauses: Vec<Clause>,
}

impl Query {
    fn matches(&self, doc: &Value) -> bool {
        let mut results = self.clauses.iter().map(|c| c.matches(doc));
        if self.any {
            results.any(|r| r)
        } else {
            results.all(|r| r)
        }
    }
}

impl Clause {
    fn matches(&self, doc: &Value) -> bool {
        let mut target = doc;
        for (i, seg) in self.field.iter().enumerate() {
            target = &target[seg.as_str()];
            // Invoices moved `subscription` under `parent.subscription_details`.
            if i == 0 && seg == "subscription" && target.is_null() {
                target = &doc["parent"]["subscription_details"]["subscription"];
            }
        }
        let result = match self.op.as_str() {
            ":" => match (&self.value, target) {
                (Value::Null, t) => t.is_null(),
                (Value::String(v), Value::String(t)) => t.eq_ignore_ascii_case(v),
                (Value::String(v), Value::Bool(t)) => as_bool(&json!(v)) == Some(*t),
                (Value::String(v), Value::Number(t)) => as_int(&json!(v)) == t.as_i64(),
                (v, t) => v == t,
            },
            "~" => match (&self.value, target) {
                (Value::String(v), Value::String(t)) => {
                    t.to_ascii_lowercase().contains(&v.to_ascii_lowercase())
                }
                _ => false,
            },
            op => match (as_int(&self.value), target.as_i64()) {
                (Some(v), Some(t)) => match op {
                    ">" => t > v,
                    ">=" => t >= v,
                    "<" => t < v,
                    "<=" => t <= v,
                    _ => false,
                },
                _ => false,
            },
        };
        result != self.negate
    }
}

fn parse_error() -> StripeError {
    StripeError::invalid(
        "We were unable to parse your search query. Try using the format `metadata[\"key\"]:\"value\"` to query for metadata or `key:\"value\"` to query for other fields.",
    )
    .param("query")
}

fn parse_query(query: &str, resource: &str, fields: &[&str]) -> Res<Query> {
    let chars: Vec<char> = query.chars().collect();
    let mut i = 0;
    let mut clauses = Vec::new();
    let mut joiners: Vec<String> = Vec::new();
    let skip_ws = |i: &mut usize| {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
    };
    loop {
        skip_ws(&mut i);
        if i >= chars.len() {
            break;
        }
        if !clauses.is_empty() {
            let word: String = chars[i..]
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if word.eq_ignore_ascii_case("and") || word.eq_ignore_ascii_case("or") {
                joiners.push(word.to_ascii_uppercase());
                i += word.len();
                skip_ws(&mut i);
            } else {
                joiners.push("AND".into());
            }
        }
        let negate = chars.get(i) == Some(&'-');
        if negate {
            i += 1;
        }
        let start = i;
        while i < chars.len()
            && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.')
        {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        if name.is_empty() {
            return Err(parse_error());
        }
        let mut field: Vec<String> = name.split('.').map(str::to_owned).collect();
        if chars.get(i) == Some(&'[') {
            if name != "metadata" {
                return Err(parse_error());
            }
            i += 1;
            let key = read_quoted(&chars, &mut i).ok_or_else(parse_error)?;
            if chars.get(i) != Some(&']') {
                return Err(parse_error());
            }
            i += 1;
            field.push(key);
        }
        if !fields.contains(&field[0].as_str()) && !fields.contains(&name.as_str()) {
            return Err(StripeError::invalid(format!(
                "Field `{name}` is an unsupported search field for resource `{resource}`. See https://stripe.com/docs/search#query-fields-for-{} for a list of supported fields.",
                resource.replace('_', "-")
            ))
            .param("query"));
        }
        let op = if chars[i..].starts_with(&['>', '=']) {
            ">="
        } else if chars[i..].starts_with(&['<', '=']) {
            "<="
        } else {
            match chars.get(i) {
                Some(':') => ":",
                Some('~') => "~",
                Some('>') => ">",
                Some('<') => "<",
                _ => return Err(parse_error()),
            }
        };
        i += op.len();
        let value = if chars.get(i) == Some(&'"') {
            Value::String(read_quoted(&chars, &mut i).ok_or_else(parse_error)?)
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            let raw: String = chars[start..i].iter().collect();
            if raw.is_empty() {
                return Err(parse_error());
            }
            if raw == "null" {
                Value::Null
            } else {
                Value::String(raw)
            }
        };
        if op == "~" && value.as_str().is_none_or(|v| v.chars().count() < 3) {
            return Err(StripeError::invalid(
                "Substring queries (`~`) require at least 3 characters.",
            )
            .param("query"));
        }
        clauses.push(Clause {
            negate,
            field,
            op: op.to_owned(),
            value,
        });
    }
    if clauses.is_empty() {
        return Err(parse_error());
    }
    let any = joiners.iter().any(|j| j == "OR");
    if any && joiners.iter().any(|j| j == "AND") {
        return Err(StripeError::invalid(
            "Search queries cannot mix the AND and OR operators. Use only one of them.",
        )
        .param("query"));
    }
    Ok(Query { any, clauses })
}

fn read_quoted(chars: &[char], i: &mut usize) -> Option<String> {
    if chars.get(*i) != Some(&'"') {
        return None;
    }
    *i += 1;
    let mut out = String::new();
    while *i < chars.len() {
        match chars[*i] {
            '\\' if *i + 1 < chars.len() => {
                out.push(chars[*i + 1]);
                *i += 2;
            }
            '"' => {
                *i += 1;
                return Some(out);
            }
            c => {
                out.push(c);
                *i += 1;
            }
        }
    }
    None
}

/// The string form of a scalar parameter for equality filters.
pub fn eq_param(doc: &Value, field: &str, params: &Params) -> bool {
    match params.raw(field) {
        None => true,
        Some(v) => scalar_string(v).is_some_and(|s| match &doc[field] {
            Value::String(t) => *t == s,
            Value::Bool(b) => b.to_string() == s,
            Value::Number(n) => n.to_string() == s,
            _ => false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_language() {
        let q = parse_query(
            "email:\"ADA@example.com\" AND metadata[\"tier\"]:\"gold\"",
            "customers",
            &["email", "metadata", "name"],
        )
        .unwrap();
        assert!(q.matches(&json!({"email": "ada@example.com", "metadata": {"tier": "gold"}})));
        assert!(!q.matches(&json!({"email": "ada@example.com", "metadata": {"tier": "silver"}})));
        let q = parse_query(
            "name~\"harb\" OR -email:null",
            "customers",
            &["email", "name"],
        )
        .unwrap();
        assert!(q.matches(&json!({"name": "Harbor", "email": null})));
        assert!(parse_query("amount>100", "customers", &["email"]).is_err());
        assert!(
            parse_query(
                "email:a AND name:b OR name:c",
                "customers",
                &["email", "name"]
            )
            .is_err()
        );
        let q = parse_query(
            "amount>=1000 status:\"succeeded\"",
            "charges",
            &["amount", "status"],
        )
        .unwrap();
        assert!(q.matches(&json!({"amount": 1000, "status": "succeeded"})));
        assert!(!q.matches(&json!({"amount": 999, "status": "succeeded"})));
    }

    #[test]
    fn months_add_on_the_calendar() {
        // 2026-01-31T00:00:00Z + 1 month = 2026-02-28.
        assert_eq!(add_months(1_769_817_600, 1), 1_772_236_800);
        assert_eq!(add_interval(0, "year", 1), 31_536_000);
    }

    #[test]
    fn pages_round_trip() {
        assert_eq!(decode_page(&encode_page(30)), Some(30));
    }
}
