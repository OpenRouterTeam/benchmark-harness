use axum::{
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json as json_value};

/// JSON response with the given status. Field order follows the `Value`.
pub fn json(status: StatusCode, body: Value) -> Response {
    with_content_type(status, "application/json; charset=utf-8", body)
}

/// JSON:API response (used by HCP Terraform).
pub fn jsonapi(status: StatusCode, body: Value) -> Response {
    with_content_type(status, "application/vnd.api+json", body)
}

fn with_content_type(status: StatusCode, content_type: &'static str, body: Value) -> Response {
    (status, [(CONTENT_TYPE, content_type)], body.to_string()).into_response()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageParams {
    pub page: usize,
    pub per_page: usize,
}

impl PageParams {
    /// Parses `page`/`per_page` query values. Unparseable values fall back to defaults;
    /// `page` is at least 1 and `per_page` is clamped to `1..=max_per_page`.
    pub fn parse(
        page: Option<&str>,
        per_page: Option<&str>,
        default_per_page: usize,
        max_per_page: usize,
    ) -> Self {
        let page = page
            .and_then(|p| p.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        let per_page = per_page
            .and_then(|p| p.parse::<usize>().ok())
            .unwrap_or(default_per_page)
            .clamp(1, max_per_page);
        Self { page, per_page }
    }
}

pub fn paginate<T: Clone>(items: &[T], params: PageParams) -> Vec<T> {
    items
        .iter()
        .skip((params.page - 1) * params.per_page)
        .take(params.per_page)
        .cloned()
        .collect()
}

/// RFC 8288 `Link` header in GitHub's order (prev, next, last, first), or `None` for a single page.
pub fn link_header(url_without_paging: &str, params: PageParams, total: usize) -> Option<String> {
    let last = total.div_ceil(params.per_page).max(1);
    let sep = if url_without_paging.contains('?') {
        '&'
    } else {
        '?'
    };
    let link = |page: usize, rel: &str| {
        format!(
            "<{url_without_paging}{sep}page={page}&per_page={}>; rel=\"{rel}\"",
            params.per_page
        )
    };
    let mut rels = Vec::new();
    if params.page > 1 {
        rels.push(link(params.page - 1, "prev"));
    }
    if params.page < last {
        rels.push(link(params.page + 1, "next"));
        rels.push(link(last, "last"));
    }
    if params.page > 1 {
        rels.push(link(1, "first"));
    }
    (!rels.is_empty()).then(|| rels.join(", "))
}

/// Body of a GraphQL POST.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphqlRequest {
    pub query: String,
    #[serde(default)]
    pub variables: Value,
    #[serde(default)]
    pub operation_name: Option<String>,
}

pub fn graphql_data(data: Value) -> Response {
    json(StatusCode::OK, json_value!({ "data": data }))
}

pub fn graphql_errors(messages: &[&str]) -> Response {
    let errors: Vec<Value> = messages
        .iter()
        .map(|m| json_value!({ "message": m }))
        .collect();
    json(
        StatusCode::OK,
        json_value!({ "data": null, "errors": errors }),
    )
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::json;

    use super::*;

    #[test]
    fn page_params_defaults_and_clamps() {
        assert_eq!(
            PageParams::parse(None, None, 30, 100),
            PageParams {
                page: 1,
                per_page: 30
            }
        );
        assert_eq!(
            PageParams::parse(Some("3"), Some("500"), 30, 100),
            PageParams {
                page: 3,
                per_page: 100
            }
        );
        assert_eq!(
            PageParams::parse(Some("0"), Some("0"), 30, 100),
            PageParams {
                page: 1,
                per_page: 1
            }
        );
        assert_eq!(
            PageParams::parse(Some("x"), Some("y"), 30, 100),
            PageParams {
                page: 1,
                per_page: 30
            }
        );
    }

    #[test]
    fn paginate_slices_and_handles_overflow() {
        let items = [1, 2, 3, 4, 5];
        assert_eq!(
            paginate(
                &items,
                PageParams {
                    page: 2,
                    per_page: 2
                }
            ),
            vec![3, 4]
        );
        assert_eq!(
            paginate(
                &items,
                PageParams {
                    page: 3,
                    per_page: 2
                }
            ),
            vec![5]
        );
        assert!(
            paginate(
                &items,
                PageParams {
                    page: 9,
                    per_page: 2
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn link_header_matches_github_order() {
        let base = "http://world:8101/api/v3/repos/o/r/issues?state=open";
        assert_eq!(
            link_header(
                base,
                PageParams {
                    page: 1,
                    per_page: 2
                },
                2
            ),
            None
        );
        assert_eq!(
            link_header(
                base,
                PageParams {
                    page: 1,
                    per_page: 2
                },
                5
            )
            .unwrap(),
            "<http://world:8101/api/v3/repos/o/r/issues?state=open&page=2&per_page=2>; rel=\"next\", \
             <http://world:8101/api/v3/repos/o/r/issues?state=open&page=3&per_page=2>; rel=\"last\""
        );
        assert_eq!(
            link_header(
                "http://h/x",
                PageParams {
                    page: 2,
                    per_page: 2
                },
                5
            )
            .unwrap(),
            "<http://h/x?page=1&per_page=2>; rel=\"prev\", <http://h/x?page=3&per_page=2>; rel=\"next\", \
             <http://h/x?page=3&per_page=2>; rel=\"last\", <http://h/x?page=1&per_page=2>; rel=\"first\""
        );
    }

    #[tokio::test]
    async fn json_sets_content_type_and_keeps_field_order() {
        let resp = json(StatusCode::CREATED, json!({"z": 1, "a": 2}));
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(
            resp.headers()[CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], br#"{"z":1,"a":2}"#);
    }

    #[test]
    fn jsonapi_uses_vendor_content_type() {
        let resp = jsonapi(StatusCode::OK, json!({"data": []}));
        assert_eq!(resp.headers()[CONTENT_TYPE], "application/vnd.api+json");
    }

    #[test]
    fn graphql_request_parses_with_and_without_variables() {
        let full: GraphqlRequest =
            serde_json::from_value(json!({"query": "query Q { viewer { login } }", "variables": {"a": 1}, "operationName": "Q"})).unwrap();
        assert_eq!(full.operation_name.as_deref(), Some("Q"));
        assert_eq!(full.variables, json!({"a": 1}));
        let bare: GraphqlRequest =
            serde_json::from_value(json!({"query": "{ viewer { login } }"})).unwrap();
        assert_eq!(bare.variables, Value::Null);
        assert_eq!(bare.operation_name, None);
    }

    #[tokio::test]
    async fn graphql_envelopes() {
        let ok = to_bytes(
            graphql_data(json!({"viewer": {"login": "mona"}})).into_body(),
            usize::MAX,
        )
        .await
        .unwrap();
        assert_eq!(&ok[..], br#"{"data":{"viewer":{"login":"mona"}}}"#);
        let err = to_bytes(
            graphql_errors(&["Field 'x' doesn't exist"]).into_body(),
            usize::MAX,
        )
        .await
        .unwrap();
        assert_eq!(
            &err[..],
            br#"{"data":null,"errors":[{"message":"Field 'x' doesn't exist"}]}"#
        );
    }
}
