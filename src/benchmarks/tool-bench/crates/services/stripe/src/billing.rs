//! Products, prices, coupons, promotion codes, subscriptions, invoices and invoice items.

use serde_json::{Value, json};

use crate::{
    api::{Args, deleted},
    engine::{Ctx, add_interval, eq_param, price_interval, short_hash},
    error::{Res, StripeError, money},
    params::{Params, as_bool, as_int, scalar_string},
    payments::{
        check_enum, matching, previous, require_ref, retrieve, set_any, set_bool, set_int,
        set_metadata, set_str,
    },
};

// --- products -----------------------------------------------------------------------

pub fn list_products(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let ids: Vec<String> = params
        .list("ids")
        .iter()
        .filter_map(scalar_string)
        .collect();
    let active = params.bool("active")?;
    let shippable = params.bool("shippable")?;
    ctx.list("product", "/v1/products", params, &|d| {
        let r = ctx.render("product", d);
        active.is_none_or(|a| r["active"] == a)
            && shippable.is_none_or(|s| r["shippable"] == s)
            && eq_param(&r, "url", params)
            && (ids.is_empty() || ids.iter().any(|i| r["id"] == i.as_str()))
    })
}

pub fn search_products(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.search(
        "product",
        "products",
        params,
        &[
            "active",
            "created",
            "description",
            "metadata",
            "name",
            "shippable",
            "url",
        ],
    )
}

pub fn get_product(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "product", args.id(), params)
}

fn product_fields(doc: &mut Value, params: &Params) -> Res<()> {
    for key in [
        "name",
        "description",
        "statement_descriptor",
        "unit_label",
        "url",
        "tax_code",
    ] {
        set_str(doc, params, key);
    }
    for key in ["images", "marketing_features", "package_dimensions"] {
        set_any(doc, params, key);
    }
    set_bool(doc, params, "active")?;
    set_bool(doc, params, "shippable")?;
    set_metadata(doc, params);
    Ok(())
}

pub fn create_product(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    params.req_str("name")?;
    let custom = params.str("id").filter(|s| !s.is_empty());
    if let Some(id) = &custom
        && ctx.lookup("product", id).is_some()
    {
        return Err(StripeError::invalid("Product already exists.")
            .code("resource_already_exists")
            .param("id"));
    }
    let mut doc = json!({"object": "product", "metadata": {}});
    product_fields(&mut doc, params)?;
    let id = custom.unwrap_or_else(|| ctx.id("product"));
    let now = ctx.now();
    doc["id"] = json!(id);
    doc["created"] = json!(now);
    doc["updated"] = json!(now);
    if let Some(data) = params.obj("default_price_data") {
        let price_params = Params::new(data);
        let price = new_price(ctx, &price_params, &id)?;
        doc["default_price"] = price["id"].clone();
    }
    ctx.put("products", &id, doc.clone());
    ctx.emit("product.created", "product", &doc, None);
    ctx.present("product", &doc, &params.expand())
}

pub fn update_product(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("product", &id, "id")?;
    let mut doc = before.clone();
    product_fields(&mut doc, params)?;
    if let Some(price) = params.str("default_price") {
        if price.is_empty() {
            doc["default_price"] = Value::Null;
        } else {
            let p = require_ref(ctx, "price", &price, "default_price")?;
            if p["product"] != id.as_str() {
                return Err(StripeError::invalid(format!(
                    "The price `{price}` does not belong to product `{id}`."
                ))
                .param("default_price"));
            }
            doc["default_price"] = json!(price);
        }
    }
    doc["updated"] = json!(ctx.now());
    ctx.put("products", &id, doc.clone());
    ctx.emit(
        "product.updated",
        "product",
        &doc,
        Some(previous(&before, &doc)),
    );
    ctx.present("product", &doc, &params.expand())
}

// --- prices -------------------------------------------------------------------------

pub fn list_prices(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let active = params.bool("active")?;
    let lookup_keys: Vec<String> = params
        .list("lookup_keys")
        .iter()
        .filter_map(scalar_string)
        .collect();
    let interval = params
        .obj("recurring")
        .and_then(|r| r.get("interval").and_then(scalar_string));
    check_enum(params, "type", &["one_time", "recurring"])?;
    ctx.list("price", "/v1/prices", params, &|d| {
        let r = ctx.render("price", d);
        active.is_none_or(|a| r["active"] == a)
            && eq_param(&r, "currency", params)
            && eq_param(&r, "product", params)
            && eq_param(&r, "type", params)
            && (lookup_keys.is_empty() || lookup_keys.iter().any(|k| r["lookup_key"] == k.as_str()))
            && interval
                .as_deref()
                .is_none_or(|i| r["recurring"]["interval"] == i)
    })
}

pub fn search_prices(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.search(
        "price",
        "prices",
        params,
        &[
            "active",
            "created",
            "currency",
            "lookup_key",
            "metadata",
            "product",
            "type",
        ],
    )
}

pub fn get_price(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "price", args.id(), params)
}

/// Creates and stores a price for `product` from price parameters.
fn new_price(ctx: &mut Ctx, params: &Params, product: &str) -> Res<Value> {
    let currency = params.req_str("currency")?.to_ascii_lowercase();
    let unit_amount = match (
        params.int("unit_amount")?,
        params.str("unit_amount_decimal"),
    ) {
        (Some(a), _) => Some(a),
        (None, Some(d)) => d.parse::<f64>().ok().map(|f| f.round() as i64),
        (None, None) => None,
    };
    if unit_amount.is_none() && !params.has("custom_unit_amount") && !params.has("tiers") {
        return Err(StripeError::missing_param("unit_amount"));
    }
    let recurring = match params.obj("recurring") {
        None => Value::Null,
        Some(r) => {
            let r = Params::new(r);
            let interval = r
                .req_str("interval")
                .map_err(|_| StripeError::missing_param("recurring[interval]"))?;
            if !["day", "week", "month", "year"].contains(&interval.as_str()) {
                return Err(StripeError::invalid_enum(
                    "recurring[interval]",
                    &["day", "month", "week", "year"],
                ));
            }
            json!({
                "interval": interval,
                "interval_count": r.int("interval_count")?.unwrap_or(1),
                "meter": null,
                "trial_period_days": null,
                "usage_type": r.str("usage_type").unwrap_or_else(|| "licensed".into())
            })
        }
    };
    if let Some(key) = params.str("lookup_key").filter(|s| !s.is_empty()) {
        let transfer = params.bool("transfer_lookup_key")?.unwrap_or(false);
        for (k, mut other) in ctx.docs("prices") {
            if other["lookup_key"] == key.as_str() {
                if !transfer {
                    return Err(StripeError::invalid(format!(
                        "A price (`{k}`) already uses that lookup key."
                    ))
                    .param("lookup_key"));
                }
                other["lookup_key"] = Value::Null;
                ctx.put("prices", &k, other);
            }
        }
    }
    let id = ctx.id("price");
    let mut doc = json!({
        "id": id,
        "object": "price",
        "active": params.bool("active")?.unwrap_or(true),
        "billing_scheme": params.str("billing_scheme").unwrap_or_else(|| "per_unit".into()),
        "created": ctx.now(),
        "currency": currency,
        "lookup_key": params.str("lookup_key").filter(|s| !s.is_empty()),
        "metadata": {},
        "nickname": params.str("nickname"),
        "product": product,
        "recurring": recurring,
        "tax_behavior": params.str("tax_behavior").unwrap_or_else(|| "unspecified".into()),
        "type": if recurring.is_null() { "one_time" } else { "recurring" },
        "unit_amount": unit_amount,
        "unit_amount_decimal": unit_amount.map(|a| a.to_string())
    });
    set_any(&mut doc, params, "custom_unit_amount");
    set_metadata(&mut doc, params);
    ctx.put("prices", &id, doc.clone());
    ctx.emit("price.created", "price", &doc, None);
    Ok(doc)
}

pub fn create_price(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let product = match (
        params.str("product").filter(|s| !s.is_empty()),
        params.obj("product_data"),
    ) {
        (Some(p), _) => {
            require_ref(ctx, "product", &p, "product")?;
            p
        }
        (None, Some(data)) => {
            let data = Params::new(data);
            let name = data
                .req_str("name")
                .map_err(|_| StripeError::missing_param("product_data[name]"))?;
            let id = data.str("id").unwrap_or_else(|| ctx.id("product"));
            let now = ctx.now();
            let mut doc = json!({"id": id, "object": "product", "created": now, "metadata": {}, "name": name, "updated": now});
            set_metadata(&mut doc, &data);
            set_bool(&mut doc, &data, "active")?;
            set_str(&mut doc, &data, "unit_label");
            ctx.put("products", &id, doc.clone());
            ctx.emit("product.created", "product", &doc, None);
            id
        }
        (None, None) => return Err(StripeError::missing_param("product")),
    };
    let price = new_price(ctx, params, &product)?;
    ctx.present("price", &price, &params.expand())
}

pub fn update_price(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("price", &id, "id")?;
    let mut doc = before.clone();
    set_bool(&mut doc, params, "active")?;
    set_str(&mut doc, params, "nickname");
    set_str(&mut doc, params, "lookup_key");
    set_str(&mut doc, params, "tax_behavior");
    set_metadata(&mut doc, params);
    ctx.put("prices", &id, doc.clone());
    ctx.emit(
        "price.updated",
        "price",
        &doc,
        Some(previous(&before, &doc)),
    );
    ctx.present("price", &doc, &params.expand())
}

// --- coupons and promotion codes ----------------------------------------------------

pub fn list_coupons(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("coupon", "/v1/coupons", params, &|_| true)
}

pub fn get_coupon(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "coupon", args.id(), params)
}

pub fn create_coupon(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let percent = params.float("percent_off")?;
    let amount = params.int("amount_off")?;
    match (percent, amount) {
        (None, None) => {
            return Err(
                StripeError::invalid("Must provide percent_off or amount_off.")
                    .code("parameter_missing"),
            );
        }
        (Some(_), Some(_)) => {
            return Err(StripeError::invalid(
                "Received both percent_off and amount_off parameters. Please pass in only one.",
            ));
        }
        _ => {}
    }
    if amount.is_some() && params.str("currency").is_none() {
        return Err(
            StripeError::invalid("You must pass currency when passing amount_off")
                .param("currency"),
        );
    }
    if let Some(p) = percent
        && !(0.01..=100.0).contains(&p)
    {
        return Err(StripeError::invalid(format!(
            "This value must be greater than or equal to 0.01 (it currently is '{p}')."
        ))
        .param("percent_off"));
    }
    check_enum(params, "duration", &["forever", "once", "repeating"])?;
    let duration = params.str("duration").unwrap_or_else(|| "once".into());
    let months = params.int("duration_in_months")?;
    if duration == "repeating" && months.is_none() {
        return Err(StripeError::invalid(
            "The duration_in_months param must be set when creating a coupon with a repeating duration",
        )
        .param("duration"));
    }
    let custom = params.str("id").filter(|s| !s.is_empty());
    if let Some(id) = &custom
        && ctx.lookup("coupon", id).is_some()
    {
        return Err(StripeError::invalid("Coupon already exists.").code("resource_already_exists"));
    }
    let now = ctx.now();
    if let Some(redeem_by) = params.int("redeem_by")?
        && redeem_by <= now
    {
        return Err(StripeError::invalid(format!(
                "The parameter `redeem_by` expects a unix timestamp representing a date and time in the future. You specified the value `{redeem_by}` which is in the past."
            ))
            .param("redeem_by"));
    }
    let id = custom.unwrap_or_else(|| ctx.id("coupon"));
    let mut doc = json!({
        "id": id,
        "object": "coupon",
        "amount_off": amount,
        "created": now,
        "currency": params.str("currency").map(|c| c.to_ascii_lowercase()),
        "duration": duration,
        "duration_in_months": if duration == "repeating" { json!(months) } else { Value::Null },
        "max_redemptions": params.int("max_redemptions")?,
        "metadata": {},
        "name": params.str("name"),
        "percent_off": percent,
        "redeem_by": params.int("redeem_by")?,
        "times_redeemed": 0,
        "valid": true
    });
    set_any(&mut doc, params, "applies_to");
    set_metadata(&mut doc, params);
    ctx.put("coupons", &id, doc.clone());
    ctx.emit("coupon.created", "coupon", &doc, None);
    ctx.present("coupon", &doc, &params.expand())
}

pub fn update_coupon(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("coupon", &id, "id")?;
    let mut doc = before.clone();
    set_str(&mut doc, params, "name");
    set_metadata(&mut doc, params);
    ctx.put("coupons", &id, doc.clone());
    ctx.emit(
        "coupon.updated",
        "coupon",
        &doc,
        Some(previous(&before, &doc)),
    );
    ctx.present("coupon", &doc, &params.expand())
}

pub fn delete_coupon(ctx: &mut Ctx, args: &Args, _: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let doc = ctx.require("coupon", &id, "id")?;
    ctx.delete("coupons", &id);
    ctx.emit("coupon.deleted", "coupon", &doc, None);
    Ok(deleted(&id, "coupon"))
}

pub fn list_promotion_codes(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let active = params.bool("active")?;
    let code = params.str("code");
    let coupon = params.str("coupon");
    ctx.list("promotion_code", "/v1/promotion_codes", params, &|d| {
        let r = ctx.render("promotion_code", d);
        active.is_none_or(|a| r["active"] == a)
            && code.as_deref().is_none_or(|c| {
                r["code"]
                    .as_str()
                    .is_some_and(|rc| rc.eq_ignore_ascii_case(c))
            })
            && coupon
                .as_deref()
                .is_none_or(|c| r["promotion"]["coupon"] == c)
            && eq_param(&r, "customer", params)
    })
}

pub fn get_promotion_code(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "promotion_code", args.id(), params)
}

pub fn create_promotion_code(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let promotion = params
        .obj("promotion")
        .ok_or_else(|| StripeError::missing_param("promotion"))?;
    let coupon = promotion
        .get("coupon")
        .and_then(scalar_string)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            StripeError::invalid("You must pass promotion.coupon when passing promotion")
                .param("promotion.coupon")
        })?;
    require_ref(ctx, "coupon", &coupon, "promotion[coupon]")?;
    let code = match params.str("code").filter(|s| !s.is_empty()) {
        Some(code) => {
            if !code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(StripeError::invalid(format!(
                    "This value must match the regex pattern. (/\\A[a-zA-Z0-9\\-_]+\\z/ does not match for the value {code})."
                ))
                .param("code"));
            }
            code
        }
        None => ctx
            .raw_id("promotion_code_code", "", 8)
            .to_ascii_uppercase(),
    };
    let taken = ctx.docs("promotion_codes").into_iter().any(|(_, p)| {
        p["code"]
            .as_str()
            .is_some_and(|c| c.eq_ignore_ascii_case(&code))
            && p["active"] != false
    });
    if taken {
        return Err(StripeError::invalid(format!(
            "An active promotion code with `code: {code}` already exists."
        )));
    }
    if let Some(customer) = params.str("customer").filter(|s| !s.is_empty()) {
        require_ref(ctx, "customer", &customer, "customer")?;
    }
    let now = ctx.now();
    if let Some(expires) = params.int("expires_at")?
        && expires <= now
    {
        return Err(StripeError::invalid(format!(
                "The parameter `expires_at` expects a unix timestamp representing a date and time in the future. You specified the value `{expires}` which is in the past."
            ))
            .param("expires_at"));
    }
    let id = ctx.id("promotion_code");
    let restrictions = params.obj("restrictions").map(Value::Object);
    let mut doc = json!({
        "id": id,
        "object": "promotion_code",
        "active": params.bool("active")?.unwrap_or(true),
        "code": code,
        "created": now,
        "customer": params.str("customer").filter(|s| !s.is_empty()),
        "expires_at": params.int("expires_at")?,
        "max_redemptions": params.int("max_redemptions")?,
        "metadata": {},
        "promotion": {"coupon": coupon, "type": "coupon"},
        "times_redeemed": 0
    });
    if let Some(mut r) = restrictions {
        for (k, v) in [
            ("first_time_transaction", json!(false)),
            ("minimum_amount", Value::Null),
            ("minimum_amount_currency", Value::Null),
        ] {
            if r.get(k).is_none() {
                r[k] = v;
            } else if k == "first_time_transaction" {
                r[k] = json!(as_bool(&r[k]).unwrap_or(false));
            } else if k == "minimum_amount" {
                r[k] = json!(as_int(&r[k]));
            }
        }
        doc["restrictions"] = r;
    }
    set_metadata(&mut doc, params);
    ctx.put("promotion_codes", &id, doc.clone());
    ctx.emit("promotion_code.created", "promotion_code", &doc, None);
    ctx.present("promotion_code", &doc, &params.expand())
}

/// The coupon a discount entry names (`{coupon}`, `{promotion_code}` or `{discount}`),
/// checked for validity. Returns `(coupon_id, promotion_code_id)`.
fn discount_source(
    ctx: &Ctx,
    entry: &Value,
    index: usize,
) -> Res<(String, Option<String>, Option<String>)> {
    let get = |k: &str| {
        entry
            .get(k)
            .and_then(scalar_string)
            .filter(|s| !s.is_empty())
    };
    if let Some(existing) = get("discount") {
        let d = ctx.lookup("discount", &existing).ok_or_else(|| {
            StripeError::missing(
                "discount",
                &existing,
                &format!("discounts[{index}][discount]"),
            )
            .status(400)
        })?;
        let coupon = d["source"]["coupon"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        return Ok((
            coupon,
            d["promotion_code"].as_str().map(str::to_owned),
            Some(existing),
        ));
    }
    let (coupon, promo) = if let Some(code) = get("promotion_code") {
        let p = require_ref(
            ctx,
            "promotion_code",
            &code,
            &format!("discounts[{index}][promotion_code]"),
        )?;
        if p["active"] == false {
            return Err(StripeError::invalid(format!(
                "This promotion code cannot be redeemed because it is inactive: {code}"
            )));
        }
        (
            p["promotion"]["coupon"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            Some(code),
        )
    } else if let Some(coupon) = get("coupon") {
        (coupon, None)
    } else {
        return Err(StripeError::invalid(
            "You must specify one of `coupon`, `discount` or `promotion_code` for each discount.",
        )
        .param(&format!("discounts[{index}]")));
    };
    let c = require_ref(
        ctx,
        "coupon",
        &coupon,
        &format!("discounts[{index}][coupon]"),
    )?;
    let rendered = ctx.render("coupon", &c);
    let expired = rendered["valid"] == false
        || rendered["redeem_by"]
            .as_i64()
            .is_some_and(|r| r <= ctx.now());
    if expired {
        return Err(StripeError::invalid(format!(
            "Coupon {coupon} is expired and cannot be applied."
        ))
        .code("coupon_expired"));
    }
    let used_up = rendered["max_redemptions"]
        .as_i64()
        .is_some_and(|m| rendered["times_redeemed"].as_i64().unwrap_or(0) >= m);
    if used_up {
        return Err(StripeError::invalid(format!(
            "Coupon {coupon} is used up and cannot be applied."
        ))
        .code("coupon_expired"));
    }
    Ok((coupon, promo, None))
}

/// Replaces a subscription's discounts with `entries`. Discount documents are keyed
/// `<subscription>:<coupon>` so graders can address them without knowing generated ids.
fn apply_discounts(ctx: &mut Ctx, sub: &mut Value, entries: &[Value]) -> Res<()> {
    let sub_id = sub["id"].as_str().unwrap_or_default().to_owned();
    let customer = sub["customer"].clone();
    let mut resolved = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        resolved.push(discount_source(ctx, entry, i)?);
    }
    let mut keep = Vec::new();
    let mut ids = Vec::new();
    let now = ctx.now();
    for (coupon, promo, _) in resolved {
        let key = format!("{sub_id}:{coupon}");
        if keep.contains(&key) {
            continue;
        }
        let id = match ctx.get("discounts", &key) {
            Some(existing) => existing["id"].as_str().unwrap_or_default().to_owned(),
            None => {
                let id = ctx.id("discount");
                let c = ctx.render("coupon", &ctx.lookup("coupon", &coupon).expect("checked"));
                let end = match c["duration"].as_str() {
                    Some("repeating") => {
                        json!(add_interval(
                            now,
                            "month",
                            c["duration_in_months"].as_i64().unwrap_or(1)
                        ))
                    }
                    _ => Value::Null,
                };
                let doc = json!({
                    "id": id,
                    "object": "discount",
                    "checkout_session": null,
                    "customer": customer,
                    "end": end,
                    "invoice": null,
                    "invoice_item": null,
                    "promotion_code": promo,
                    "source": {"coupon": coupon, "type": "coupon"},
                    "start": now,
                    "subscription": sub_id,
                    "subscription_item": null
                });
                ctx.put("discounts", &key, doc);
                if let Some(mut cdoc) = ctx.lookup("coupon", &coupon) {
                    cdoc["times_redeemed"] = json!(c["times_redeemed"].as_i64().unwrap_or(0) + 1);
                    ctx.put("coupons", &coupon, cdoc);
                }
                if let Some(code) = &promo
                    && let Some(mut pdoc) = ctx.lookup("promotion_code", code)
                {
                    pdoc["times_redeemed"] =
                        json!(pdoc["times_redeemed"].as_i64().unwrap_or(0) + 1);
                    ctx.put("promotion_codes", code, pdoc);
                }
                id
            }
        };
        keep.push(key);
        ids.push(id);
    }
    let prefix = format!("{sub_id}:");
    for (key, _) in ctx.docs("discounts") {
        if key.starts_with(&prefix) && !keep.contains(&key) {
            ctx.delete("discounts", &key);
        }
    }
    sub["discounts"] = json!(ids);
    Ok(())
}

// --- subscriptions ------------------------------------------------------------------

const SUB_STATUSES: &[&str] = &[
    "active",
    "all",
    "canceled",
    "ended",
    "incomplete",
    "incomplete_expired",
    "past_due",
    "paused",
    "trialing",
    "unpaid",
];

pub fn list_subscriptions(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    check_enum(params, "status", SUB_STATUSES)?;
    check_enum(
        params,
        "collection_method",
        &["charge_automatically", "send_invoice"],
    )?;
    let status = params.str("status");
    let price = params.str("price");
    ctx.list("subscription", "/v1/subscriptions", params, &|d| {
        let r = ctx.render("subscription", d);
        let s = r["status"].as_str().unwrap_or_default();
        let status_ok = match status.as_deref() {
            None => s != "canceled",
            Some("all") => true,
            Some("ended") => s == "canceled" || s == "incomplete_expired",
            Some(want) => s == want,
        };
        status_ok
            && eq_param(&r, "customer", params)
            && eq_param(&r, "collection_method", params)
            && eq_param(&r, "test_clock", params)
            && price.as_deref().is_none_or(|p| {
                r["items"]["data"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|i| i["price"]["id"] == p))
            })
    })
}

pub fn search_subscriptions(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.search(
        "subscription",
        "subscriptions",
        params,
        &["canceled_at", "created", "metadata", "status"],
    )
}

pub fn get_subscription(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "subscription", args.id(), params)
}

pub fn list_subscription_items(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let sub_id = params.req_str("subscription")?;
    let sub = require_ref(ctx, "subscription", &sub_id, "subscription")?;
    let rendered = ctx.render("subscription", &sub);
    let items = rendered["items"]["data"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    ctx.page(
        "subscription_item",
        &format!("/v1/subscription_items?subscription={sub_id}"),
        items,
        params,
    )
}

const NO_PAYMENT_METHOD: &str = "This customer has no attached payment source or default payment method. Please consider adding a default payment method. For more information, visit https://stripe.com/docs/billing/subscriptions/payment-methods-setting#payment-method-priority.";

fn customer_payment_method(ctx: &Ctx, customer: &Value) -> Option<String> {
    customer["invoice_settings"]["default_payment_method"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            let id = customer["id"].as_str()?;
            matching(ctx, "payment_methods", |pm| pm["customer"] == id)
                .first()
                .and_then(|pm| pm["id"].as_str().map(str::to_owned))
        })
}

/// A new subscription item for `price` (stored with the price id; rendering expands it).
fn new_item(ctx: &mut Ctx, sub_id: &str, entry: &Value, index: usize, now: i64) -> Res<Value> {
    let get = |k: &str| {
        entry
            .get(k)
            .and_then(scalar_string)
            .filter(|s| !s.is_empty())
    };
    let price_id = get("price")
        .ok_or_else(|| StripeError::missing_param(&format!("items[{index}][price]")))?;
    let price = require_ref(ctx, "price", &price_id, &format!("items[{index}][price]"))?;
    let price = ctx.render("price", &price);
    if price["type"] != "recurring" {
        return Err(StripeError::invalid("The price specified is set to `type=one_time` but this field only accepts prices with `type=recurring`.".to_string())
        .param(&format!("items[{index}][price]")));
    }
    let quantity = match entry.get("quantity") {
        None => 1,
        Some(q) => as_int(q).ok_or_else(|| {
            StripeError::invalid_integer(&format!("items[{index}][quantity]"), &q.to_string())
        })?,
    };
    let (interval, count) = price_interval(&price);
    let id = ctx.raw_id("subscription_item", "si_", 14);
    Ok(json!({
        "id": id,
        "object": "subscription_item",
        "created": now,
        "current_period_end": add_interval(now, &interval, count),
        "current_period_start": now,
        "metadata": entry.get("metadata").cloned().unwrap_or_else(|| json!({})),
        "price": price_id,
        "quantity": quantity,
        "subscription": sub_id
    }))
}

pub fn create_subscription(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let customer_id = params.req_str("customer")?;
    let customer = require_ref(ctx, "customer", &customer_id, "customer")?;
    if customer["deleted"] == true {
        return Err(StripeError::missing("customer", &customer_id, "customer").status(400));
    }
    let entries = params.list("items");
    if entries.is_empty() {
        return Err(StripeError::missing_param("items"));
    }
    check_enum(
        params,
        "collection_method",
        &["charge_automatically", "send_invoice"],
    )?;
    check_enum(
        params,
        "payment_behavior",
        &[
            "allow_incomplete",
            "default_incomplete",
            "error_if_incomplete",
            "pending_if_incomplete",
        ],
    )?;
    let collection = params
        .str("collection_method")
        .unwrap_or_else(|| "charge_automatically".into());
    let days_until_due = params.int("days_until_due")?;
    if collection == "send_invoice" && days_until_due.is_none() {
        return Err(StripeError::invalid(
            "If invoice collection method is 'send_invoice', you must specify 'days_until_due'.",
        ));
    }
    if collection != "send_invoice" && days_until_due.is_some() {
        return Err(StripeError::invalid(
            "You can only specify 'days_until_due' if invoice collection method is 'send_invoice'.",
        ));
    }
    let now = ctx.now();
    let sub_id = ctx.id("subscription");
    let mut items = Vec::new();
    let mut prices_seen: Vec<String> = Vec::new();
    let mut currency: Option<String> = None;
    for (i, entry) in entries.iter().enumerate() {
        let item = new_item(ctx, &sub_id, entry, i, now)?;
        let price_id = item["price"].as_str().unwrap_or_default().to_owned();
        if prices_seen.contains(&price_id) {
            return Err(StripeError::invalid(format!(
                "Cannot create a Subscription with multiple Subscription Items with the same Price: {price_id}"
            ))
            .param("plan"));
        }
        let price = ctx.render("price", &ctx.lookup("price", &price_id).expect("checked"));
        let c = price["currency"].as_str().unwrap_or("usd").to_owned();
        if let Some(existing) = &currency
            && *existing != c
        {
            return Err(StripeError::invalid(format!(
                "The price specified only supports `{c}`. This doesn't match the expected currency: `{existing}`."
            )));
        }
        currency = Some(c);
        prices_seen.push(price_id);
        items.push(item);
    }
    let currency = currency.unwrap_or_else(|| "usd".into());
    let trial_end = match (params.int("trial_period_days")?, params.raw("trial_end")) {
        (Some(days), _) => Some(now + days * 86_400),
        (None, Some(v)) if v.as_str() == Some("now") => None,
        (None, Some(v)) => {
            let t = as_int(v)
                .ok_or_else(|| StripeError::invalid_integer("trial_end", &v.to_string()))?;
            if t <= now {
                return Err(StripeError::invalid(format!(
                    "The parameter `trial_end` expects a unix timestamp representing a date and time in the future. You specified the value `{t}` which is in the past."
                ))
                .param("trial_end"));
            }
            Some(t)
        }
        (None, None) => None,
    };
    let default_pm = params
        .str("default_payment_method")
        .filter(|s| !s.is_empty());
    if let Some(pm) = &default_pm {
        require_ref(ctx, "payment_method", pm, "default_payment_method")?;
    }
    let pm = default_pm
        .clone()
        .or_else(|| customer_payment_method(ctx, &customer));
    let behavior = params
        .str("payment_behavior")
        .unwrap_or_else(|| "allow_incomplete".into());
    if trial_end.is_none()
        && collection == "charge_automatically"
        && pm.is_none()
        && behavior == "error_if_incomplete"
    {
        return Err(StripeError::invalid(NO_PAYMENT_METHOD).code("resource_missing"));
    }
    if let Some(end) = trial_end {
        for item in &mut items {
            item["current_period_end"] = json!(end);
        }
    }
    let mut sub = json!({
        "id": sub_id,
        "object": "subscription",
        "billing_cycle_anchor": trial_end.unwrap_or(now),
        "cancel_at": params.int("cancel_at")?,
        "cancel_at_period_end": params.bool("cancel_at_period_end")?.unwrap_or(false),
        "collection_method": collection,
        "created": now,
        "currency": currency,
        "customer": customer_id,
        "days_until_due": days_until_due,
        "default_payment_method": default_pm,
        "description": params.str("description"),
        "discounts": [],
        "items": {"object": "list", "data": items},
        "metadata": {},
        "start_date": now,
        "status": "active",
        "trial_end": trial_end,
        "trial_start": trial_end.map(|_| now)
    });
    set_metadata(&mut sub, params);
    let discounts = params.list("discounts");
    if !discounts.is_empty() {
        apply_discounts(ctx, &mut sub, &discounts)?;
    }
    if sub["cancel_at_period_end"] == true {
        sub["cancel_at"] = json!(period_end(ctx, &sub));
        sub["canceled_at"] = json!(now);
    }
    // The first invoice: $0 during a trial, otherwise the first period.
    let invoice = subscription_invoice(ctx, &sub, "subscription_create", trial_end.is_some())?;
    let invoice_id = invoice["id"].as_str().unwrap_or_default().to_owned();
    sub["latest_invoice"] = json!(invoice_id);
    let status = if trial_end.is_some() {
        "trialing"
    } else if collection == "send_invoice" {
        "active"
    } else if let Some(pm) = &pm {
        pay_invoice(ctx, &invoice_id, pm)?;
        "active"
    } else {
        "incomplete"
    };
    sub["status"] = json!(status);
    ctx.put("subscriptions", &sub_id, sub.clone());
    ctx.emit("customer.subscription.created", "subscription", &sub, None);
    ctx.present("subscription", &sub, &params.expand())
}

fn period_end(ctx: &Ctx, sub: &Value) -> i64 {
    let rendered = ctx.render("subscription", sub);
    rendered["items"]["data"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .filter_map(|i| i["current_period_end"].as_i64())
                .max()
        })
        .unwrap_or_else(|| ctx.now())
}

const CANCELED_FIELDS: &str =
    "A canceled subscription can only update its cancellation_details and metadata.";

pub fn update_subscription(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("subscription", &id, "id")?;
    let rendered = ctx.render("subscription", &before);
    let status = rendered["status"].as_str().unwrap_or("active").to_owned();
    if (status == "canceled" || status == "incomplete_expired")
        && params
            .keys()
            .any(|k| !["cancellation_details", "metadata", "expand"].contains(&k))
    {
        return Err(
            StripeError::invalid(CANCELED_FIELDS).code("invalid_canceled_subscription_fields")
        );
    }
    check_enum(
        params,
        "collection_method",
        &["charge_automatically", "send_invoice"],
    )?;
    check_enum(
        params,
        "proration_behavior",
        &["always_invoice", "create_prorations", "none"],
    )?;
    let now = ctx.now();
    let mut sub = before.clone();
    set_metadata(&mut sub, params);
    set_str(&mut sub, params, "description");
    if let Some(pm) = params.str("default_payment_method") {
        if pm.is_empty() {
            sub["default_payment_method"] = Value::Null;
        } else {
            require_ref(ctx, "payment_method", &pm, "default_payment_method")?;
            sub["default_payment_method"] = json!(pm);
        }
    }
    if let Some(method) = params.str("collection_method") {
        sub["collection_method"] = json!(method);
    }
    if params.has("days_until_due") {
        let method = sub["collection_method"]
            .as_str()
            .unwrap_or("charge_automatically");
        if method != "send_invoice" {
            return Err(StripeError::invalid(
                "You can only specify 'days_until_due' if invoice collection method is 'send_invoice'.",
            ));
        }
        set_int(&mut sub, params, "days_until_due")?;
    }
    if let Some(details) = params.obj("cancellation_details") {
        let mut current = rendered["cancellation_details"].clone();
        for (k, v) in details {
            current[k.as_str()] = if v.as_str() == Some("") {
                Value::Null
            } else {
                v
            };
        }
        sub["cancellation_details"] = current;
    }
    if params.has("pause_collection") {
        match params.obj("pause_collection") {
            Some(p) => {
                let behavior = p
                    .get("behavior")
                    .and_then(scalar_string)
                    .unwrap_or_default();
                if !["keep_as_draft", "mark_uncollectible", "void"].contains(&behavior.as_str()) {
                    return Err(StripeError::invalid_enum(
                        "pause_collection[behavior]",
                        &["keep_as_draft", "mark_uncollectible", "void"],
                    ));
                }
                sub["pause_collection"] = json!({
                    "behavior": behavior,
                    "resumes_at": p.get("resumes_at").and_then(as_int)
                });
            }
            None => sub["pause_collection"] = Value::Null,
        }
    }
    // Items: update by id, add by price, remove with `deleted`.
    let entries = params.list("items");
    if !entries.is_empty() {
        let mut items: Vec<Value> = sub["items"]["data"].as_array().cloned().unwrap_or_default();
        for (i, entry) in entries.iter().enumerate() {
            let item_id = entry.get("id").and_then(scalar_string);
            let delete = entry.get("deleted").and_then(as_bool).unwrap_or(false);
            match item_id {
                Some(item_id) => {
                    let Some(pos) = items.iter().position(|it| it["id"] == item_id.as_str()) else {
                        return Err(StripeError::invalid(format!(
                            "No such subscription item: '{item_id}'"
                        ))
                        .code("resource_missing")
                        .param(&format!("items[{i}][id]")));
                    };
                    if delete {
                        items.remove(pos);
                        continue;
                    }
                    if let Some(price) = entry.get("price").and_then(scalar_string) {
                        require_ref(ctx, "price", &price, &format!("items[{i}][price]"))?;
                        items[pos]["price"] = json!(price);
                    }
                    if let Some(q) = entry.get("quantity") {
                        let q = as_int(q).ok_or_else(|| {
                            StripeError::invalid_integer(
                                &format!("items[{i}][quantity]"),
                                &q.to_string(),
                            )
                        })?;
                        items[pos]["quantity"] = json!(q);
                    }
                    if let Some(m) = entry.get("metadata") {
                        let mut current = items[pos]
                            .get("metadata")
                            .cloned()
                            .unwrap_or_else(|| json!({}));
                        crate::params::apply_metadata(&mut current, Some(m));
                        items[pos]["metadata"] = current;
                    }
                }
                None => {
                    let item = new_item(ctx, &id, entry, i, now)?;
                    let price = item["price"].as_str().unwrap_or_default();
                    if let Some(existing) = items
                        .iter()
                        .find(|it| it["price"] == price || it["price"]["id"] == price)
                    {
                        return Err(StripeError::invalid(format!(
                            "A new item with Price {price} can't be added to this Subscription because an existing Subscription Item {} is already using that Price. If you want to update the existing item (e.g., to adjust the quantity), pass the existing Subscription Item's `id` in your update request: `items[0][id]={}`.",
                            existing["id"].as_str().unwrap_or_default(),
                            existing["id"].as_str().unwrap_or_default()
                        ))
                        .param("plan"));
                    }
                    items.push(item);
                }
            }
        }
        sub["items"] = json!({"object": "list", "data": items});
    }
    if params.has("discounts") {
        let entries = params.list("discounts");
        apply_discounts(ctx, &mut sub, &entries)?;
    }
    if let Some(trial_end) = params.raw("trial_end") {
        if trial_end.as_str() == Some("now") {
            sub["trial_end"] = json!(now);
            if status == "trialing" {
                sub["status"] = json!("active");
            }
        } else {
            let t = as_int(trial_end)
                .ok_or_else(|| StripeError::invalid_integer("trial_end", &trial_end.to_string()))?;
            sub["trial_end"] = json!(t);
        }
    }
    // Scheduling or clearing a cancellation.
    let cancel_flag = params.bool("cancel_at_period_end")?;
    let cancel_at_param = params.raw("cancel_at").cloned();
    if let Some(flag) = cancel_flag {
        sub["cancel_at_period_end"] = json!(flag);
        sub["cancel_at"] = if flag {
            json!(period_end(ctx, &sub))
        } else {
            Value::Null
        };
    }
    if let Some(at) = &cancel_at_param {
        sub["cancel_at"] = if at.as_str() == Some("") {
            Value::Null
        } else {
            json!(
                as_int(at)
                    .ok_or_else(|| StripeError::invalid_integer("cancel_at", &at.to_string()))?
            )
        };
    }
    let requested = match (cancel_flag, &cancel_at_param) {
        (Some(true), _) => Some(true),
        (_, Some(at)) if at.as_str() != Some("") => Some(true),
        (Some(false), _) | (_, Some(_)) => Some(false),
        (None, None) => None,
    };
    if let Some(requested) = requested {
        let mut details = sub
            .get("cancellation_details")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| rendered["cancellation_details"].clone());
        if requested {
            sub["canceled_at"] = json!(now);
            details["reason"] = json!("cancellation_requested");
        } else {
            sub["canceled_at"] = Value::Null;
            sub["cancel_at_period_end"] = json!(false);
            sub["cancel_at"] = Value::Null;
            details["reason"] = Value::Null;
        }
        sub["cancellation_details"] = details;
    }
    ctx.put("subscriptions", &id, sub.clone());
    let prev = previous(
        &ctx.render("subscription", &before),
        &ctx.render("subscription", &sub),
    );
    ctx.emit(
        "customer.subscription.updated",
        "subscription",
        &sub,
        Some(prev),
    );
    ctx.present("subscription", &sub, &params.expand())
}

pub fn cancel_subscription(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("subscription", &id, "id")?;
    let rendered = ctx.render("subscription", &before);
    if rendered["status"] == "canceled" || rendered["status"] == "incomplete_expired" {
        return Err(StripeError::missing("subscription", &id, "id"));
    }
    params.bool("invoice_now")?;
    params.bool("prorate")?;
    let now = ctx.now();
    let mut sub = before.clone();
    let mut details = rendered["cancellation_details"].clone();
    details["reason"] = json!("cancellation_requested");
    if let Some(given) = params.obj("cancellation_details") {
        for (k, v) in given {
            details[k.as_str()] = if v.as_str() == Some("") {
                Value::Null
            } else {
                v
            };
        }
    }
    sub["cancellation_details"] = details;
    sub["status"] = json!("canceled");
    sub["canceled_at"] = json!(now);
    sub["ended_at"] = json!(now);
    ctx.put("subscriptions", &id, sub.clone());
    ctx.emit("customer.subscription.deleted", "subscription", &sub, None);
    ctx.present("subscription", &sub, &params.expand())
}

// --- invoices -----------------------------------------------------------------------

fn invoice_subscription(doc: &Value) -> &Value {
    let nested = &doc["parent"]["subscription_details"]["subscription"];
    if nested.is_null() {
        &doc["subscription"]
    } else {
        nested
    }
}

pub fn list_invoices(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    check_enum(
        params,
        "status",
        &["draft", "open", "paid", "uncollectible", "void"],
    )?;
    let subscription = params.str("subscription");
    let due = params.raw("due_date").cloned();
    ctx.list("invoice", "/v1/invoices", params, &|d| {
        let r = ctx.render("invoice", d);
        eq_param(&r, "customer", params)
            && eq_param(&r, "status", params)
            && eq_param(&r, "collection_method", params)
            && subscription
                .as_deref()
                .is_none_or(|s| *invoice_subscription(&r) == s)
            && due
                .as_ref()
                .is_none_or(|f| crate::engine::range_match(&r["due_date"], f))
    })
}

pub fn search_invoices(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.search(
        "invoice",
        "invoices",
        params,
        &[
            "created",
            "currency",
            "customer",
            "last_finalization_error_code",
            "last_finalization_error_type",
            "metadata",
            "number",
            "receipt_number",
            "status",
            "subscription",
            "total",
        ],
    )
}

pub fn get_invoice(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "invoice", args.id(), params)
}

pub fn list_invoice_lines(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let doc = ctx.require("invoice", &id, "id")?;
    let rendered = ctx.render("invoice", &doc);
    let lines = rendered["lines"]["data"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    ctx.page(
        "line_item",
        &format!("/v1/invoices/{id}/lines"),
        lines,
        params,
    )
}

/// Stored totals follow the lines (drafts only change through their lines).
fn recompute(doc: &mut Value) {
    let lines = doc["lines"]["data"].as_array().cloned().unwrap_or_default();
    let subtotal: i64 = lines.iter().filter_map(|l| l["amount"].as_i64()).sum();
    let discount: i64 = doc["total_discount_amounts"]
        .as_array()
        .map(|a| a.iter().filter_map(|d| d["amount"].as_i64()).sum())
        .unwrap_or(0);
    let total = subtotal - discount;
    let paid = doc["amount_paid"].as_i64().unwrap_or(0);
    doc["subtotal"] = json!(subtotal);
    doc["subtotal_excluding_tax"] = json!(subtotal);
    doc["total"] = json!(total);
    doc["total_excluding_tax"] = json!(total);
    doc["amount_due"] = json!(total.max(0));
    doc["amount_paid"] = json!(paid);
    doc["amount_remaining"] = json!((total - paid).max(0));
    if let Some(list) = doc["lines"].as_object_mut() {
        list.insert("total_count".into(), json!(lines.len()));
    }
}

fn customer_snapshot(customer: &Value) -> Value {
    json!({
        "customer_address": customer["address"],
        "customer_email": customer["email"],
        "customer_name": customer["name"],
        "customer_phone": customer["phone"],
        "customer_shipping": customer["shipping"],
        "customer_tax_exempt": customer.get("tax_exempt").cloned().unwrap_or_else(|| json!("none")),
        "customer_tax_ids": []
    })
}

/// Builds and stores the invoice for a subscription period.
fn subscription_invoice(ctx: &mut Ctx, sub: &Value, reason: &str, trial: bool) -> Res<Value> {
    let invoice_id = ctx.id("invoice");
    let now = ctx.now();
    let sub_id = sub["id"].as_str().unwrap_or_default().to_owned();
    let currency = sub["currency"].as_str().unwrap_or("usd").to_owned();
    let customer_id = sub["customer"].as_str().unwrap_or_default().to_owned();
    let customer = ctx.lookup("customer", &customer_id).unwrap_or_default();
    let mut lines = Vec::new();
    for item in sub["items"]["data"].as_array().cloned().unwrap_or_default() {
        let price = ctx.render(
            "price",
            &ctx.lookup("price", item["price"].as_str().unwrap_or_default())
                .unwrap_or_default(),
        );
        let quantity = item["quantity"].as_i64().unwrap_or(1);
        let unit = if trial {
            0
        } else {
            price["unit_amount"].as_i64().unwrap_or(0)
        };
        let period_end = item["current_period_end"].as_i64().unwrap_or(now);
        let product_name = ctx
            .lookup("product", price["product"].as_str().unwrap_or_default())
            .and_then(|p| p["name"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let (interval, _) = price_interval(&price);
        let description = if trial {
            format!("Trial period for {product_name}")
        } else {
            format!(
                "{quantity} × {product_name} (at {} / {interval})",
                money(&currency, price["unit_amount"].as_i64().unwrap_or(0))
            )
        };
        let id = ctx.raw_id("line_item", "il_", 24);
        lines.push(json!({
            "id": id,
            "object": "line_item",
            "amount": unit * quantity,
            "currency": currency,
            "description": description,
            "invoice": invoice_id,
            "parent": {
                "invoice_item_details": null,
                "subscription_item_details": {
                    "invoice_item": null,
                    "proration": false,
                    "proration_details": {"credited_items": null},
                    "subscription": sub_id,
                    "subscription_item": item["id"]
                },
                "type": "subscription_item_details"
            },
            "period": {"end": period_end, "start": item["current_period_start"].as_i64().unwrap_or(now)},
            "pricing": {"price_details": {"price": price["id"], "product": price["product"]}, "type": "price_details", "unit_amount_decimal": unit.to_string()},
            "quantity": quantity,
            "subtotal": unit * quantity
        }));
    }
    let subtotal: i64 = lines.iter().filter_map(|l| l["amount"].as_i64()).sum();
    let mut discount_amounts = Vec::new();
    let mut remaining = subtotal;
    for discount_id in sub["discounts"].as_array().cloned().unwrap_or_default() {
        let Some(d) = ctx.lookup("discount", discount_id.as_str().unwrap_or_default()) else {
            continue;
        };
        let Some(c) = ctx.lookup("coupon", d["source"]["coupon"].as_str().unwrap_or_default())
        else {
            continue;
        };
        let off = match (c["percent_off"].as_f64(), c["amount_off"].as_i64()) {
            (Some(p), _) => ((remaining as f64) * p / 100.0).round() as i64,
            (None, Some(a)) => a.min(remaining),
            _ => 0,
        };
        remaining -= off;
        discount_amounts.push(json!({"amount": off, "discount": discount_id}));
    }
    let mut invoice = json!({
        "id": invoice_id,
        "object": "invoice",
        "account_country": ctx.account()["country"],
        "account_name": ctx.account_name(),
        "amount_paid": 0,
        "auto_advance": false,
        "billing_reason": reason,
        "collection_method": sub["collection_method"],
        "created": now,
        "currency": currency,
        "customer": customer_id,
        "discounts": sub["discounts"],
        "lines": {"object": "list", "data": lines},
        "metadata": {},
        "parent": {
            "quote_details": null,
            "subscription_details": {"metadata": sub["metadata"], "subscription": sub_id},
            "type": "subscription_details"
        },
        "period_end": now,
        "period_start": now,
        "status": "draft",
        "total_discount_amounts": discount_amounts
    });
    crate::engine::merge(&mut invoice, &customer_snapshot(&customer));
    recompute(&mut invoice);
    if sub["collection_method"] == "send_invoice" {
        invoice["due_date"] = json!(now + sub["days_until_due"].as_i64().unwrap_or(30) * 86_400);
    }
    ctx.put("invoices", &invoice_id, invoice.clone());
    ctx.emit("invoice.created", "invoice", &invoice, None);
    finalize(ctx, &invoice_id)?;
    Ok(ctx.lookup("invoice", &invoice_id).expect("stored"))
}

/// Marks an open invoice paid with a new PaymentIntent and Charge on `payment_method`.
fn pay_invoice(ctx: &mut Ctx, invoice_id: &str, payment_method: &str) -> Res<()> {
    let mut invoice = ctx.lookup("invoice", invoice_id).expect("stored");
    let amount = invoice["amount_remaining"].as_i64().unwrap_or(0);
    if amount <= 0 || invoice["status"] != "open" {
        return Ok(());
    }
    let now = ctx.now();
    let currency = invoice["currency"].as_str().unwrap_or("usd").to_owned();
    let customer = invoice["customer"].clone();
    let pi_id = ctx.id("payment_intent");
    let charge_id = ctx.id("charge");
    let txn_id = ctx.id("balance_transaction");
    let pm = ctx
        .lookup("payment_method", payment_method)
        .unwrap_or_default();
    let description = if invoice["billing_reason"] == "subscription_create" {
        "Subscription creation".to_owned()
    } else {
        "Payment for Invoice".to_owned()
    };
    let fee = (amount as f64 * 0.029).round() as i64 + 30;
    let charge = json!({
        "id": charge_id,
        "object": "charge",
        "amount": amount,
        "amount_captured": amount,
        "amount_refunded": 0,
        "balance_transaction": txn_id,
        "billing_details": pm["billing_details"],
        "calculated_statement_descriptor": ctx.account_name().to_ascii_uppercase(),
        "captured": true,
        "created": now,
        "currency": currency,
        "customer": customer,
        "description": description,
        "paid": true,
        "payment_intent": pi_id,
        "payment_method": payment_method,
        "payment_method_details": {"card": pm["card"], "type": pm["type"].as_str().unwrap_or("card")},
        "refunded": false,
        "status": "succeeded"
    });
    let intent = json!({
        "id": pi_id,
        "object": "payment_intent",
        "amount": amount,
        "amount_received": amount,
        "created": now,
        "currency": currency,
        "customer": customer,
        "description": description,
        "latest_charge": charge_id,
        "payment_method": payment_method,
        "status": "succeeded"
    });
    let txn = json!({
        "id": txn_id,
        "object": "balance_transaction",
        "amount": amount,
        "available_on": now + 2 * 86_400,
        "created": now,
        "currency": currency,
        "description": description,
        "fee": fee,
        "fee_details": [{"amount": fee, "application": null, "currency": currency, "description": "Stripe processing fees", "type": "stripe_fee"}],
        "net": amount - fee,
        "reporting_category": "charge",
        "source": charge_id,
        "status": "pending",
        "type": "charge"
    });
    let payment_id = ctx.id("invoice_payment");
    let payment = json!({
        "id": payment_id,
        "object": "invoice_payment",
        "amount_paid": amount,
        "amount_requested": amount,
        "created": now,
        "currency": currency,
        "invoice": invoice_id,
        "is_default": true,
        "payment": {"payment_intent": pi_id, "type": "payment_intent"},
        "status": "paid",
        "status_transitions": {"canceled_at": null, "paid_at": now}
    });
    ctx.put("payment_intents", &pi_id, intent.clone());
    ctx.put("charges", &charge_id, charge.clone());
    ctx.put("balance_transactions", &txn_id, txn);
    ctx.put("invoice_payments", &payment_id, payment);
    invoice["status"] = json!("paid");
    invoice["amount_paid"] = json!(amount);
    invoice["amount_remaining"] = json!(0);
    invoice["attempt_count"] = json!(1);
    invoice["attempted"] = json!(true);
    invoice["status_transitions"]["paid_at"] = json!(now);
    ctx.put("invoices", invoice_id, invoice.clone());
    ctx.emit("payment_intent.succeeded", "payment_intent", &intent, None);
    ctx.emit("charge.succeeded", "charge", &charge, None);
    ctx.emit("invoice.paid", "invoice", &invoice, None);
    Ok(())
}

pub fn create_invoice(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let customer_id = params.req_str("customer")?;
    let customer = require_ref(ctx, "customer", &customer_id, "customer")?;
    if customer["deleted"] == true {
        return Err(StripeError::missing("customer", &customer_id, "customer").status(400));
    }
    check_enum(
        params,
        "collection_method",
        &["charge_automatically", "send_invoice"],
    )?;
    check_enum(
        params,
        "pending_invoice_items_behavior",
        &["exclude", "include"],
    )?;
    let collection = params
        .str("collection_method")
        .unwrap_or_else(|| "charge_automatically".into());
    let days = params.int("days_until_due")?;
    let due_date = params.int("due_date")?;
    let now = ctx.now();
    if collection == "send_invoice" && days.is_none() && due_date.is_none() {
        return Err(StripeError::invalid(
            "If invoice collection method is 'send_invoice', you must specify 'due_date' or 'days_until_due'.",
        ));
    }
    if collection != "send_invoice" && (days.is_some() || due_date.is_some()) {
        return Err(StripeError::invalid(
            "You can only specify 'due_date' or 'days_until_due' if invoice collection method is 'send_invoice'.",
        ));
    }
    let currency = params
        .str("currency")
        .or_else(|| customer["currency"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "usd".into())
        .to_ascii_lowercase();
    let id = ctx.id("invoice");
    let mut lines = Vec::new();
    if params.str("pending_invoice_items_behavior").as_deref() == Some("include") {
        for (key, mut item) in ctx.docs("invoiceitems") {
            if item["customer"] == customer_id.as_str()
                && item["invoice"].is_null()
                && item["currency"].as_str().unwrap_or("usd") == currency
            {
                item["invoice"] = json!(id);
                lines.push(invoice_item_line(ctx, &id, &item));
                ctx.put("invoiceitems", &key, item);
            }
        }
    }
    let mut invoice = json!({
        "id": id,
        "object": "invoice",
        "account_country": ctx.account()["country"],
        "account_name": ctx.account_name(),
        "amount_paid": 0,
        "auto_advance": params.bool("auto_advance")?.unwrap_or(false),
        "billing_reason": "manual",
        "collection_method": collection,
        "created": now,
        "currency": currency,
        "customer": customer_id,
        "days_until_due": days,
        "description": params.str("description"),
        "due_date": due_date.or(days.map(|d| now + d * 86_400)),
        "footer": params.str("footer"),
        "lines": {"object": "list", "data": lines},
        "metadata": {},
        "number": null,
        "period_end": now,
        "period_start": now,
        "statement_descriptor": params.str("statement_descriptor"),
        "status": "draft"
    });
    crate::engine::merge(&mut invoice, &customer_snapshot(&customer));
    set_any(&mut invoice, params, "custom_fields");
    set_any(&mut invoice, params, "rendering");
    set_metadata(&mut invoice, params);
    if let Some(pm) = params
        .str("default_payment_method")
        .filter(|s| !s.is_empty())
    {
        require_ref(ctx, "payment_method", &pm, "default_payment_method")?;
        invoice["default_payment_method"] = json!(pm);
    }
    recompute(&mut invoice);
    ctx.put("invoices", &id, invoice.clone());
    ctx.emit("invoice.created", "invoice", &invoice, None);
    ctx.present("invoice", &invoice, &params.expand())
}

const FINALIZED_EDITABLE: &[&str] = &[
    "auto_advance",
    "custom_fields",
    "default_payment_method",
    "description",
    "days_until_due",
    "due_date",
    "expand",
    "footer",
    "metadata",
    "rendering",
    "statement_descriptor",
];

pub fn update_invoice(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("invoice", &id, "id")?;
    let status = ctx.render("invoice", &before)["status"]
        .as_str()
        .unwrap_or("draft")
        .to_owned();
    if status != "draft" {
        if let Some(key) = params.keys().find(|k| !FINALIZED_EDITABLE.contains(k)) {
            return Err(
                StripeError::invalid("Finalized invoices can't be updated in this way").param(key),
            );
        }
        if status != "open" && params.keys().any(|k| k != "metadata" && k != "expand") {
            return Err(
                StripeError::invalid("Finalized invoices can't be updated in this way").param(
                    params
                        .keys()
                        .find(|k| *k != "metadata")
                        .unwrap_or("description"),
                ),
            );
        }
    }
    check_enum(
        params,
        "collection_method",
        &["charge_automatically", "send_invoice"],
    )?;
    let mut doc = before.clone();
    for key in [
        "description",
        "footer",
        "statement_descriptor",
        "collection_method",
    ] {
        set_str(&mut doc, params, key);
    }
    for key in [
        "custom_fields",
        "rendering",
        "shipping_details",
        "shipping_cost",
    ] {
        set_any(&mut doc, params, key);
    }
    set_bool(&mut doc, params, "auto_advance")?;
    set_int(&mut doc, params, "due_date")?;
    if let Some(days) = params.int("days_until_due")? {
        doc["days_until_due"] = json!(days);
        doc["due_date"] = json!(doc["created"].as_i64().unwrap_or(ctx.now()) + days * 86_400);
    }
    if let Some(pm) = params.str("default_payment_method") {
        if pm.is_empty() {
            doc["default_payment_method"] = Value::Null;
        } else {
            require_ref(ctx, "payment_method", &pm, "default_payment_method")?;
            doc["default_payment_method"] = json!(pm);
        }
    }
    set_metadata(&mut doc, params);
    ctx.put("invoices", &id, doc.clone());
    ctx.emit(
        "invoice.updated",
        "invoice",
        &doc,
        Some(previous(&before, &doc)),
    );
    ctx.present("invoice", &doc, &params.expand())
}

/// Draft → open: number, finalized_at, hosted URLs; a $0 invoice is paid at once.
fn finalize(ctx: &mut Ctx, id: &str) -> Res<Value> {
    let stored = ctx.lookup("invoice", id).expect("caller checked");
    let mut doc = ctx.render("invoice", &stored);
    let mut out = stored.clone();
    let now = ctx.now();
    let customer_id = doc["customer"].as_str().unwrap_or_default().to_owned();
    let number = match ctx.lookup("customer", &customer_id) {
        Some(mut customer) => {
            let rendered = ctx.render("customer", &customer);
            let prefix = rendered["invoice_prefix"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    customer_id
                        .trim_start_matches("cus_")
                        .chars()
                        .take(8)
                        .collect::<String>()
                        .to_ascii_uppercase()
                });
            let seq = rendered["next_invoice_sequence"].as_i64().unwrap_or(1);
            customer["next_invoice_sequence"] = json!(seq + 1);
            ctx.put("customers", &customer_id, customer);
            format!("{prefix}-{seq:04}")
        }
        None => format!("DRAFT-{:04}", 1),
    };
    for key in [
        "amount_due",
        "amount_paid",
        "amount_remaining",
        "subtotal",
        "subtotal_excluding_tax",
        "total",
        "total_excluding_tax",
        "lines",
    ] {
        out[key] = doc[key].clone();
    }
    if let Some(list) = out["lines"].as_object_mut() {
        list.remove("url");
    }
    out["number"] = json!(number);
    out["status"] = json!("open");
    out["effective_at"] = json!(now);
    out["ending_balance"] = json!(0);
    let mut transitions = doc["status_transitions"].clone();
    transitions["finalized_at"] = json!(now);
    if doc["collection_method"] == "send_invoice" && doc["due_date"].is_null() {
        out["due_date"] = json!(now + doc["days_until_due"].as_i64().unwrap_or(30) * 86_400);
    }
    if doc["amount_due"].as_i64().unwrap_or(0) == 0 {
        out["status"] = json!("paid");
        transitions["paid_at"] = json!(now);
    }
    out["status_transitions"] = transitions;
    doc = out;
    ctx.put("invoices", id, doc.clone());
    ctx.emit("invoice.finalized", "invoice", &doc, None);
    if doc["status"] == "paid" {
        ctx.emit("invoice.paid", "invoice", &doc, None);
    }
    Ok(doc)
}

pub fn finalize_invoice(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let doc = ctx.require("invoice", &id, "id")?;
    if ctx.render("invoice", &doc)["status"] != "draft" {
        return Err(StripeError::invalid(
            "This invoice is already finalized, you can't re-finalize a non-draft invoice.",
        ));
    }
    let mut finalized = finalize(ctx, &id)?;
    if let Some(auto) = params.bool("auto_advance")? {
        finalized["auto_advance"] = json!(auto);
        ctx.put("invoices", &id, finalized.clone());
    }
    ctx.present("invoice", &finalized, &params.expand())
}

pub fn void_invoice(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let stored = ctx.require("invoice", &id, "id")?;
    let status = ctx.render("invoice", &stored)["status"].clone();
    if status == "paid" {
        return Err(StripeError::invalid(
            "Invoices with `paid` payments cannot be voided.",
        ));
    }
    if status != "open" && status != "uncollectible" {
        return Err(StripeError::invalid(
            "You can only pass in open invoices. This invoice isn't open.",
        ));
    }
    let mut doc = stored.clone();
    let now = ctx.now();
    doc["status"] = json!("void");
    doc["amount_remaining"] = json!(0);
    let mut transitions = ctx.render("invoice", &stored)["status_transitions"].clone();
    transitions["voided_at"] = json!(now);
    doc["status_transitions"] = transitions;
    ctx.put("invoices", &id, doc.clone());
    ctx.emit(
        "invoice.voided",
        "invoice",
        &doc,
        Some(json!({"status": status})),
    );
    ctx.present("invoice", &doc, &params.expand())
}

pub fn mark_uncollectible(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let stored = ctx.require("invoice", &id, "id")?;
    let status = ctx.render("invoice", &stored)["status"].clone();
    if status == "uncollectible" {
        return Err(StripeError::invalid(
            "This invoice has already been marked uncollectible.",
        ));
    }
    if status != "open" {
        return Err(StripeError::invalid(
            "You can only pass in open invoices. This invoice isn't open.",
        ));
    }
    let mut doc = stored.clone();
    doc["status"] = json!("uncollectible");
    let mut transitions = ctx.render("invoice", &stored)["status_transitions"].clone();
    transitions["marked_uncollectible_at"] = json!(ctx.now());
    doc["status_transitions"] = transitions;
    ctx.put("invoices", &id, doc.clone());
    ctx.emit(
        "invoice.marked_uncollectible",
        "invoice",
        &doc,
        Some(json!({"status": status})),
    );
    ctx.present("invoice", &doc, &params.expand())
}

pub fn create_preview(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let sub_id = params.str("subscription").filter(|s| !s.is_empty());
    let customer_param = params.str("customer").filter(|s| !s.is_empty());
    let (customer_id, sub) = match (&customer_param, &sub_id) {
        (_, Some(s)) => {
            let sub = require_ref(ctx, "subscription", s, "subscription")?;
            (
                sub["customer"].as_str().unwrap_or_default().to_owned(),
                Some(sub),
            )
        }
        (Some(c), None) => (c.clone(), None),
        (None, None) => return Err(StripeError::missing_param("customer")),
    };
    let customer = require_ref(ctx, "customer", &customer_id, "customer")?;
    let now = ctx.now();
    let preview_id = format!(
        "upcoming_in_{}",
        short_hash(&format!("{customer_id}{sub_id:?}"))
    );
    let mut lines = Vec::new();
    let currency;
    if let Some(sub) = &sub {
        let rendered = ctx.render("subscription", sub);
        currency = rendered["currency"].as_str().unwrap_or("usd").to_owned();
        for item in rendered["items"]["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let price = &item["price"];
            let quantity = item["quantity"].as_i64().unwrap_or(1);
            let amount = price["unit_amount"].as_i64().unwrap_or(0) * quantity;
            let start = item["current_period_end"].as_i64().unwrap_or(now);
            let (interval, count) = price_interval(price);
            let product_name = ctx
                .lookup("product", price["product"].as_str().unwrap_or_default())
                .and_then(|p| p["name"].as_str().map(str::to_owned))
                .unwrap_or_default();
            lines.push(json!({
                "id": format!("il_tmp_{}", short_hash(item["id"].as_str().unwrap_or_default())),
                "object": "line_item",
                "amount": amount,
                "currency": currency,
                "description": format!("{quantity} × {product_name} (at {} / {interval})", money(&currency, price["unit_amount"].as_i64().unwrap_or(0))),
                "invoice": preview_id,
                "parent": {"invoice_item_details": null, "subscription_item_details": {"invoice_item": null, "proration": false, "proration_details": {"credited_items": null}, "subscription": sub["id"], "subscription_item": item["id"]}, "type": "subscription_item_details"},
                "period": {"end": add_interval(start, &interval, count), "start": start},
                "pricing": {"price_details": {"price": price["id"], "product": price["product"]}, "type": "price_details", "unit_amount_decimal": price["unit_amount"].as_i64().unwrap_or(0).to_string()},
                "quantity": quantity,
                "subtotal": amount
            }));
        }
    } else {
        currency = params
            .str("currency")
            .or_else(|| customer["currency"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "usd".into());
        for (_, item) in ctx.docs("invoiceitems") {
            if item["customer"] == customer_id.as_str() && item["invoice"].is_null() {
                lines.push(invoice_item_line(ctx, &preview_id, &item));
            }
        }
    }
    let mut invoice = json!({
        "id": preview_id,
        "object": "invoice",
        "account_country": ctx.account()["country"],
        "account_name": ctx.account_name(),
        "amount_paid": 0,
        "billing_reason": if sub.is_some() { "upcoming" } else { "manual" },
        "created": now,
        "currency": currency,
        "customer": customer_id,
        "lines": {"object": "list", "data": lines},
        "status": "draft"
    });
    if let Some(sub) = &sub {
        invoice["parent"] = json!({"quote_details": null, "subscription_details": {"metadata": sub["metadata"], "subscription": sub["id"]}, "type": "subscription_details"});
        invoice["discounts"] = sub["discounts"].clone();
    }
    crate::engine::merge(&mut invoice, &customer_snapshot(&customer));
    recompute(&mut invoice);
    ctx.present("invoice", &invoice, &params.expand())
}

pub fn list_invoice_payments(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let payment = params.obj("payment");
    ctx.list("invoice_payment", "/v1/invoice_payments", params, &|d| {
        eq_param(d, "invoice", params)
            && eq_param(d, "status", params)
            && payment.as_ref().is_none_or(|p| {
                p.iter().all(|(k, v)| {
                    scalar_string(v).is_some_and(|s| d["payment"][k.as_str()] == s.as_str())
                })
            })
    })
}

// --- invoice items ------------------------------------------------------------------

fn invoice_item_line(ctx: &mut Ctx, invoice_id: &str, item: &Value) -> Value {
    let rendered = ctx.render("invoiceitem", item);
    let id = ctx.raw_id("line_item", "il_", 24);
    json!({
        "id": id,
        "object": "line_item",
        "amount": rendered["amount"],
        "currency": rendered["currency"],
        "description": rendered["description"],
        "discountable": rendered["discountable"],
        "invoice": invoice_id,
        "metadata": rendered["metadata"],
        "parent": {
            "invoice_item_details": {
                "invoice_item": rendered["id"],
                "proration": false,
                "proration_details": {"credited_items": null},
                "subscription": null
            },
            "subscription_item_details": null,
            "type": "invoice_item_details"
        },
        "period": rendered["period"],
        "pricing": rendered["pricing"],
        "quantity": rendered["quantity"],
        "subtotal": rendered["amount"]
    })
}

pub fn list_invoiceitems(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let pending = params.bool("pending")?;
    ctx.list("invoiceitem", "/v1/invoiceitems", params, &|d| {
        eq_param(d, "customer", params)
            && eq_param(d, "invoice", params)
            && pending.is_none_or(|p| d["invoice"].is_null() == p)
    })
}

pub fn get_invoiceitem(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "invoiceitem", args.id(), params)
}

fn editable_invoice(ctx: &Ctx, invoice_id: &str, param: &str) -> Res<Value> {
    let invoice = require_ref(ctx, "invoice", invoice_id, param)?;
    if ctx.render("invoice", &invoice)["status"] != "draft" {
        return Err(
            StripeError::invalid("Invalid invoice: This invoice is no longer editable")
                .code("invoice_not_editable"),
        );
    }
    Ok(invoice)
}

/// `(amount, quantity, pricing)` from an invoice item's amount/pricing/price_data parameters.
fn item_amount(ctx: &mut Ctx, params: &Params, currency: &str) -> Res<Option<(i64, i64, Value)>> {
    let quantity = params.int("quantity")?.unwrap_or(1);
    if let Some(amount) = params.int("amount")? {
        return Ok(Some((amount, 1, Value::Null)));
    }
    if let Some(pricing) = params.obj("pricing") {
        let price_id = pricing
            .get("price")
            .and_then(scalar_string)
            .ok_or_else(|| StripeError::missing_param("pricing[price]"))?;
        let price = ctx.render(
            "price",
            &require_ref(ctx, "price", &price_id, "pricing[price]")?,
        );
        let unit = price["unit_amount"].as_i64().unwrap_or(0);
        return Ok(Some((
            unit * quantity,
            quantity,
            json!({"price_details": {"price": price_id, "product": price["product"]}, "type": "price_details", "unit_amount_decimal": unit.to_string()}),
        )));
    }
    if let Some(data) = params.obj("price_data") {
        let data = Params::new(data);
        let unit = data
            .int("unit_amount")?
            .ok_or_else(|| StripeError::missing_param("price_data[unit_amount]"))?;
        let product = data
            .req_str("product")
            .map_err(|_| StripeError::missing_param("price_data[product]"))?;
        require_ref(ctx, "product", &product, "price_data[product]")?;
        let price = new_price(
            ctx,
            &{
                let mut p = Params::new(serde_json::Map::new());
                p.insert(
                    "currency",
                    json!(data.str("currency").unwrap_or_else(|| currency.to_owned())),
                );
                p.insert("unit_amount", json!(unit));
                p
            },
            &product,
        )?;
        return Ok(Some((
            unit * quantity,
            quantity,
            json!({"price_details": {"price": price["id"], "product": product}, "type": "price_details", "unit_amount_decimal": unit.to_string()}),
        )));
    }
    Ok(None)
}

pub fn create_invoiceitem(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let customer_id = params.req_str("customer")?;
    let customer = require_ref(ctx, "customer", &customer_id, "customer")?;
    let invoice_id = params.str("invoice").filter(|s| !s.is_empty());
    let invoice = match &invoice_id {
        Some(i) => Some(editable_invoice(ctx, i, "invoice")?),
        None => None,
    };
    let currency = params
        .str("currency")
        .or_else(|| {
            invoice
                .as_ref()
                .and_then(|i| i["currency"].as_str().map(str::to_owned))
        })
        .or_else(|| customer["currency"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "usd".into())
        .to_ascii_lowercase();
    let Some((amount, quantity, pricing)) = item_amount(ctx, params, &currency)? else {
        return Err(StripeError::missing_param("amount"));
    };
    let now = ctx.now();
    let id = ctx.id("invoiceitem");
    let period = params
        .obj("period")
        .map(|p| json!({"end": p.get("end").and_then(as_int).unwrap_or(now), "start": p.get("start").and_then(as_int).unwrap_or(now)}))
        .unwrap_or_else(|| json!({"end": now, "start": now}));
    let mut item = json!({
        "id": id,
        "object": "invoiceitem",
        "amount": amount,
        "currency": currency,
        "customer": customer_id,
        "date": now,
        "description": params.str("description"),
        "discountable": params.bool("discountable")?.unwrap_or(true),
        "discounts": [],
        "invoice": invoice_id,
        "metadata": {},
        "parent": null,
        "period": period,
        "pricing": pricing,
        "proration": false,
        "quantity": quantity,
        "tax_rates": []
    });
    if let Some(sub) = params.str("subscription").filter(|s| !s.is_empty()) {
        require_ref(ctx, "subscription", &sub, "subscription")?;
        item["parent"] = json!({"subscription_details": {"subscription": sub, "subscription_item": null}, "type": "subscription_details"});
    }
    set_metadata(&mut item, params);
    ctx.put("invoiceitems", &id, item.clone());
    if let (Some(invoice_id), Some(mut invoice)) = (invoice_id, invoice) {
        let line = invoice_item_line(ctx, &invoice_id, &item);
        if let Some(lines) = invoice["lines"]["data"].as_array_mut() {
            lines.push(line);
        } else {
            invoice["lines"] = json!({"object": "list", "data": [line]});
        }
        recompute(&mut invoice);
        ctx.put("invoices", &invoice_id, invoice);
    }
    ctx.emit("invoiceitem.created", "invoiceitem", &item, None);
    ctx.present("invoiceitem", &item, &params.expand())
}

pub fn update_invoiceitem(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("invoiceitem", &id, "id")?;
    let invoice = match before["invoice"].as_str() {
        Some(i) => Some(editable_invoice(ctx, i, "invoice")?),
        None => None,
    };
    let mut item = before.clone();
    let currency = item["currency"].as_str().unwrap_or("usd").to_owned();
    if let Some((amount, quantity, pricing)) = item_amount(ctx, params, &currency)? {
        item["amount"] = json!(amount);
        item["quantity"] = json!(quantity);
        if !pricing.is_null() {
            item["pricing"] = pricing;
        }
    } else if let Some(q) = params.int("quantity")? {
        let unit =
            item["amount"].as_i64().unwrap_or(0) / item["quantity"].as_i64().unwrap_or(1).max(1);
        item["quantity"] = json!(q);
        item["amount"] = json!(unit * q);
    }
    set_str(&mut item, params, "description");
    set_bool(&mut item, params, "discountable")?;
    set_any(&mut item, params, "period");
    set_metadata(&mut item, params);
    ctx.put("invoiceitems", &id, item.clone());
    if let Some(mut invoice) = invoice {
        let invoice_id = invoice["id"].as_str().unwrap_or_default().to_owned();
        let new_line = invoice_item_line(ctx, &invoice_id, &item);
        if let Some(lines) = invoice["lines"]["data"].as_array_mut() {
            for line in lines.iter_mut() {
                if line["parent"]["invoice_item_details"]["invoice_item"] == id.as_str() {
                    let line_id = line["id"].clone();
                    *line = new_line.clone();
                    line["id"] = line_id;
                }
            }
        }
        recompute(&mut invoice);
        ctx.put("invoices", &invoice_id, invoice);
    }
    ctx.emit(
        "invoiceitem.updated",
        "invoiceitem",
        &item,
        Some(previous(&before, &item)),
    );
    ctx.present("invoiceitem", &item, &params.expand())
}
