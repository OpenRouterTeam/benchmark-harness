//! Variable coercion rules for the scalar, enum and input-object types the Admin
//! mutations take. Unknown input fields, wrong scalar kinds, bad enum values and
//! malformed global IDs are rejected the way Shopify's GraphQL layer rejects them.

use serde_json::Value;

use crate::{graphql::TypeRef, model::is_gid};

/// `(path, explanation)` of each problem found.
pub type Problems = Vec<(Vec<Value>, String)>;

fn input_fields(type_name: &str) -> Option<&'static [(&'static str, &'static str)]> {
    const ADDRESS: &[(&str, &str)] = &[
        ("address1", "String"),
        ("address2", "String"),
        ("city", "String"),
        ("company", "String"),
        ("country", "String"),
        ("countryCode", "String"),
        ("firstName", "String"),
        ("lastName", "String"),
        ("phone", "String"),
        ("province", "String"),
        ("provinceCode", "String"),
        ("zip", "String"),
        ("id", "ID"),
    ];
    Some(match type_name {
        "OrderInput" => &[
            ("id", "ID"),
            ("tags", "[String]"),
            ("email", "String"),
            ("note", "String"),
            ("customAttributes", "[AttributeInput]"),
            ("metafields", "[MetafieldInput]"),
            ("shippingAddress", "MailingAddressInput"),
            ("poNumber", "String"),
            ("localizedFields", "Any"),
        ],
        "AttributeInput" => &[("key", "String"), ("value", "String")],
        "MetafieldInput" => &[
            ("id", "ID"),
            ("namespace", "String"),
            ("key", "String"),
            ("value", "String"),
            ("type", "String"),
            ("compareDigest", "String"),
        ],
        "MailingAddressInput" => ADDRESS,
        "CustomerInput" => &[
            ("id", "ID"),
            ("firstName", "String"),
            ("lastName", "String"),
            ("email", "String"),
            ("phone", "String"),
            ("tags", "[String]"),
            ("note", "String"),
            ("taxExempt", "Boolean"),
            ("taxExemptions", "[String]"),
            ("metafields", "[MetafieldInput]"),
            ("addresses", "[MailingAddressInput]"),
            ("locale", "String"),
            ("emailMarketingConsent", "Any"),
            ("smsMarketingConsent", "Any"),
        ],
        "ProductCreateInput" => &[
            ("title", "String"),
            ("descriptionHtml", "String"),
            ("handle", "String"),
            ("vendor", "String"),
            ("productType", "String"),
            ("tags", "[String]"),
            ("status", "ProductStatus"),
            ("seo", "SEOInput"),
            ("metafields", "[MetafieldInput]"),
            ("productOptions", "[OptionCreateInput]"),
            ("collectionsToJoin", "[ID]"),
            ("category", "ID"),
            ("giftCard", "Boolean"),
            ("giftCardTemplateSuffix", "String"),
            ("templateSuffix", "String"),
            ("requiresSellingPlan", "Boolean"),
            ("combinedListingRole", "String"),
        ],
        "ProductInput" => &[
            ("id", "ID"),
            ("title", "String"),
            ("descriptionHtml", "String"),
            ("handle", "String"),
            ("vendor", "String"),
            ("productType", "String"),
            ("tags", "[String]"),
            ("status", "ProductStatus"),
            ("seo", "SEOInput"),
            ("metafields", "[MetafieldInput]"),
            ("productOptions", "[OptionCreateInput]"),
            ("collectionsToJoin", "[ID]"),
            ("collectionsToLeave", "[ID]"),
            ("redirectNewHandle", "Boolean"),
            ("category", "ID"),
            ("giftCard", "Boolean"),
            ("templateSuffix", "String"),
            ("requiresSellingPlan", "Boolean"),
        ],
        "SEOInput" => &[("title", "String"), ("description", "String")],
        "OptionCreateInput" => &[
            ("name", "String"),
            ("position", "Int"),
            ("values", "[OptionValueCreateInput]"),
            ("linkedMetafield", "Any"),
        ],
        "OptionValueCreateInput" => &[("name", "String"), ("linkedMetafieldValue", "String")],
        "OptionUpdateInput" => &[
            ("id", "ID"),
            ("name", "String"),
            ("position", "Int"),
            ("linkedMetafield", "Any"),
        ],
        "ProductVariantsBulkInput" => &[
            ("id", "ID"),
            ("price", "Money"),
            ("compareAtPrice", "Money"),
            ("barcode", "String"),
            ("taxable", "Boolean"),
            ("inventoryItem", "InventoryItemInput"),
            ("optionValues", "[VariantOptionValueInput]"),
            ("inventoryPolicy", "ProductVariantInventoryPolicy"),
            ("inventoryQuantities", "Any"),
            ("mediaId", "ID"),
            ("mediaSrc", "[String]"),
            ("metafields", "[MetafieldInput]"),
            ("requiresComponents", "Boolean"),
            ("taxCode", "String"),
        ],
        "InventoryItemInput" => &[
            ("sku", "String"),
            ("tracked", "Boolean"),
            ("cost", "Decimal"),
            ("requiresShipping", "Boolean"),
            ("harmonizedSystemCode", "String"),
            ("countryCodeOfOrigin", "String"),
            ("provinceCodeOfOrigin", "String"),
            ("measurement", "Any"),
        ],
        "VariantOptionValueInput" => &[
            ("optionName", "String"),
            ("name", "String"),
            ("optionId", "ID"),
            ("id", "ID"),
            ("linkedMetafieldValue", "String"),
        ],
        "ProductDeleteInput" => &[("id", "ID")],
        "CreateMediaInput" => &[
            ("originalSource", "String"),
            ("alt", "String"),
            ("mediaContentType", "String"),
        ],
        _ => return None,
    })
}

fn enum_values(type_name: &str) -> Option<&'static [&'static str]> {
    Some(match type_name {
        "ProductStatus" => &["ACTIVE", "ARCHIVED", "DRAFT"],
        "ProductVariantsBulkCreateStrategy" => &[
            "DEFAULT",
            "PRESERVE_STANDALONE_VARIANT",
            "REMOVE_STANDALONE_VARIANT",
        ],
        "ProductVariantInventoryPolicy" => &["CONTINUE", "DENY"],
        "ProductSortKeys" => &[
            "CREATED_AT",
            "ID",
            "INVENTORY_TOTAL",
            "PRODUCT_TYPE",
            "PUBLISHED_AT",
            "RELEVANCE",
            "TITLE",
            "UPDATED_AT",
            "VENDOR",
        ],
        "OrderSortKeys" => &[
            "CREATED_AT",
            "CUSTOMER_NAME",
            "FINANCIAL_STATUS",
            "FULFILLMENT_STATUS",
            "ID",
            "ORDER_NUMBER",
            "PROCESSED_AT",
            "RELEVANCE",
            "TOTAL_PRICE",
            "UPDATED_AT",
        ],
        "CustomerSortKeys" => &[
            "CREATED_AT",
            "ID",
            "LOCATION",
            "NAME",
            "RELEVANCE",
            "UPDATED_AT",
        ],
        _ => return None,
    })
}

fn parse_mini(ty: &str) -> TypeRef {
    match ty.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        Some(inner) => TypeRef::List(Box::new(parse_mini(inner))),
        None => TypeRef::Named(ty.to_owned()),
    }
}

fn show(v: &Value) -> String {
    v.to_string()
}

pub fn validate(ty: &TypeRef, value: &Value, path: &mut Vec<Value>, problems: &mut Problems) {
    match ty {
        TypeRef::NonNull(inner) => {
            if value.is_null() {
                problems.push((path.clone(), "Expected value to not be null".to_owned()));
            } else {
                validate(inner, value, path, problems);
            }
        }
        _ if value.is_null() => {}
        TypeRef::List(inner) => match value {
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    path.push(Value::from(i));
                    validate(inner, item, path, problems);
                    path.pop();
                }
            }
            other => validate(inner, other, path, problems),
        },
        TypeRef::Named(name) => validate_named(name, value, path, problems),
    }
}

fn validate_named(name: &str, value: &Value, path: &mut Vec<Value>, problems: &mut Problems) {
    let mut fail = |msg: String| problems.push((path.clone(), msg));
    match name {
        "Int" => {
            if !value.is_i64() && !value.is_u64() {
                fail(format!("Could not coerce value {} to Int", show(value)));
            }
        }
        "Float" => {
            if !value.is_number() {
                fail(format!("Could not coerce value {} to Float", show(value)));
            }
        }
        "Boolean" => {
            if !value.is_boolean() {
                fail(format!("Could not coerce value {} to Boolean", show(value)));
            }
        }
        "String" | "URL" | "HTML" | "DateTime" | "FormattedString" => {
            if !value.is_string() {
                fail(format!("Could not coerce value {} to String", show(value)));
            }
        }
        "Money" | "Decimal" => {
            let ok = match value {
                Value::Number(_) => true,
                Value::String(s) => crate::model::parse_cents(s).is_some(),
                _ => false,
            };
            if !ok {
                fail(format!("Could not coerce value {} to {name}", show(value)));
            }
        }
        "ID" => match value {
            Value::String(s) if is_gid(s) => {}
            Value::String(s) => fail(format!("Invalid global id '{s}'")),
            other => fail(format!("Could not coerce value {} to ID", show(other))),
        },
        "Any" => {}
        _ => {
            if let Some(values) = enum_values(name) {
                let ok = value.as_str().is_some_and(|s| values.contains(&s));
                if !ok {
                    fail(format!(
                        "Expected {} to be one of: {}",
                        show(value),
                        values.join(", ")
                    ));
                }
            } else if let Some(fields) = input_fields(name) {
                let Some(obj) = value.as_object() else {
                    fail(format!(
                        "Expected {} to be a key-value object.",
                        show(value)
                    ));
                    return;
                };
                for (key, v) in obj {
                    path.push(Value::String(key.clone()));
                    match fields.iter().find(|(f, _)| f == key) {
                        None => {
                            problems.push((path.clone(), format!("Field is not defined on {name}")))
                        }
                        Some((_, ty)) => validate(&parse_mini(ty), v, path, problems),
                    }
                    path.pop();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn problems(ty: &str, v: Value) -> Problems {
        let doc = crate::graphql::parse(&format!("query Q($v: {ty}) {{ a }}")).unwrap();
        let mut out = Vec::new();
        validate(&doc.operations[0].vars[0].ty, &v, &mut Vec::new(), &mut out);
        out
    }

    #[test]
    fn rejects_unknown_fields_bad_ids_and_enums() {
        assert!(
            problems(
                "OrderInput!",
                json!({"id": "gid://shopify/Order/1", "tags": ["a"]})
            )
            .is_empty()
        );
        assert_eq!(
            problems(
                "OrderInput!",
                json!({"id": "gid://shopify/Order/1", "foo": 1})
            ),
            vec![(
                vec![json!("foo")],
                "Field is not defined on OrderInput".to_owned()
            )]
        );
        assert_eq!(
            problems("ID!", json!("123")),
            vec![(vec![], "Invalid global id '123'".to_owned())]
        );
        assert_eq!(
            problems("ID!", Value::Null)[0].1,
            "Expected value to not be null"
        );
        assert_eq!(
            problems(
                "ProductInput!",
                json!({"id": "gid://shopify/Product/1", "status": "LIVE"})
            )[0]
            .0,
            vec![json!("status")]
        );
        assert!(
            problems("[ProductVariantsBulkInput!]!", json!([{"price": "4x"}]))[0]
                .1
                .contains("Money")
        );
        assert!(problems("Int!", json!(10)).is_empty());
    }
}
