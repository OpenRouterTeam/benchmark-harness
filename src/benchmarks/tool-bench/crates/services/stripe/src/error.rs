//! Stripe's error envelope: `{"error": {"type", "message", "code"?, "param"?, "doc_url"?}}`.

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct StripeError {
    pub status: u16,
    pub kind: &'static str,
    pub message: String,
    pub code: Option<String>,
    pub param: Option<String>,
}

pub type Res<T> = Result<T, StripeError>;

impl StripeError {
    /// A 400 `invalid_request_error`.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            kind: "invalid_request_error",
            message: message.into(),
            code: None,
            param: None,
        }
    }

    pub fn code(mut self, code: &str) -> Self {
        self.code = Some(code.to_owned());
        self
    }

    pub fn param(mut self, param: &str) -> Self {
        self.param = Some(param.to_owned());
        self
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// `No such customer: 'cus_x'` (404 on a path id; use `.status(400)` for body/query ids).
    pub fn missing(object: &str, id: &str, param: &str) -> Self {
        Self::invalid(format!("No such {object}: '{id}'"))
            .code("resource_missing")
            .param(param)
            .status(404)
    }

    pub fn missing_param(param: &str) -> Self {
        Self::invalid(format!("Missing required param: {param}."))
            .code("parameter_missing")
            .param(param)
    }

    pub fn unknown_param(param: &str) -> Self {
        Self::invalid(format!("Received unknown parameter: {param}"))
            .code("parameter_unknown")
            .param(param)
    }

    pub fn invalid_integer(param: &str, value: &str) -> Self {
        Self::invalid(format!("Invalid integer: {value}"))
            .code("parameter_invalid_integer")
            .param(param)
    }

    pub fn invalid_boolean(param: &str, value: &str) -> Self {
        Self::invalid(format!(
            "Invalid boolean: {value}. Must be one of: true, false"
        ))
        .code("parameter_invalid_boolean")
        .param(param)
    }

    pub fn invalid_enum(param: &str, allowed: &[&str]) -> Self {
        Self::invalid(format!(
            "Invalid {param}: must be one of {}",
            join_or(allowed)
        ))
        .param(param)
    }

    pub fn unrecognized_url(method: &str, path: &str) -> Self {
        Self::invalid(format!(
            "Unrecognized request URL ({method}: {path}). Please see https://stripe.com/docs or we can help at https://support.stripe.com/."
        ))
        .status(404)
    }

    pub fn body(&self) -> Value {
        let mut error = Map::new();
        if let Some(code) = &self.code {
            error.insert("code".into(), json!(code));
            if self.kind == "invalid_request_error" || self.kind == "card_error" {
                error.insert(
                    "doc_url".into(),
                    json!(format!(
                        "https://stripe.com/docs/error-codes/{}",
                        code.replace('_', "-")
                    )),
                );
            }
        }
        error.insert("message".into(), json!(self.message));
        if let Some(param) = &self.param {
            error.insert("param".into(), json!(param));
        }
        error.insert("type".into(), json!(self.kind));
        json!({ "error": Value::Object(error) })
    }
}

fn join_or(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{}, or {last}", init.join(", ")),
    }
}

/// `$40.00`, the amount format Stripe uses in refund messages.
pub fn money(currency: &str, amount: i64) -> String {
    let symbol = match currency.to_ascii_lowercase().as_str() {
        "usd" | "cad" | "aud" | "nzd" | "sgd" => "$",
        "eur" => "€",
        "gbp" => "£",
        "jpy" => "¥",
        _ => "",
    };
    let sign = if amount < 0 { "-" } else { "" };
    let abs = amount.abs();
    if symbol.is_empty() {
        format!(
            "{sign}{}{}.{:02}",
            currency.to_ascii_uppercase(),
            abs / 100,
            abs % 100
        )
    } else {
        format!("{sign}{symbol}{}.{:02}", abs / 100, abs % 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_has_real_shape() {
        let body = StripeError::missing("customer", "cus_x", "id").body();
        assert_eq!(
            body,
            json!({"error": {
                "code": "resource_missing",
                "doc_url": "https://stripe.com/docs/error-codes/resource-missing",
                "message": "No such customer: 'cus_x'",
                "param": "id",
                "type": "invalid_request_error"
            }})
        );
    }

    #[test]
    fn money_formats_minor_units() {
        assert_eq!(money("usd", 4800), "$48.00");
        assert_eq!(money("eur", 5), "€0.05");
        assert_eq!(money("chf", 1234), "CHF12.34");
    }
}
