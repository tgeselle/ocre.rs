use axum::{body::Body, http::Request as HttpRequest};

use super::*;
use crate::test_util::{block_on, body_text};

fn json_of(response: Response) -> (u16, serde_json::Value) {
    let status = response.status().as_u16();
    (status, serde_json::from_str(&body_text(response)).unwrap())
}

#[test]
fn errors_are_json_and_hide_internal_details() {
    let (status, body) = json_of(ApiError::from(Error::NotFound).into_response());
    assert_eq!((status, body), (404, serde_json::json!({"error": {"status": 404, "message": "Not found"}})));
    let unauthorized = ApiError::from(Error::Unauthorized).into_response();
    assert_eq!(unauthorized.headers()["www-authenticate"], "Bearer");
    let (status, body) = json_of(unauthorized);
    assert_eq!((status, body), (401, serde_json::json!({"error": {"status": 401, "message": "Unauthorized"}})));
    let forbidden = ApiError::from(Error::Forbidden).into_response();
    assert!(!forbidden.headers().contains_key("www-authenticate"));
    assert_eq!(json_of(forbidden).0, 403);
    let (status, body) = json_of(ApiError::from(worker::Error::RustError("secret".into())).into_response());
    assert_eq!((status, body["error"]["message"].as_str()), (500, Some("Internal server error")));
    let (status, body) = json_of(ApiError::from(Error::bad_request("<b>")).into_response());
    assert_eq!((status, body["error"]["message"].as_str()), (400, Some("<b>")), "JSON needs no HTML escaping");
    let invalid = Error::Invalid(vec![
        crate::FieldError::new("title", "can't be blank"),
        crate::FieldError::new("title", "is too short (minimum is 3 characters)"),
    ]);
    let (status, body) = json_of(ApiError::from(invalid).into_response());
    assert_eq!(status, 422);
    assert_eq!(
        body["error"]["fields"],
        serde_json::json!({"title": ["can't be blank", "is too short (minimum is 3 characters)"]})
    );
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct Item {
    name: String,
}

fn post(body: &str, content_type: Option<&str>) -> Result<Json<Item>, ApiError> {
    let mut request = HttpRequest::builder().method("POST").uri("/");
    if let Some(value) = content_type {
        request = request.header("content-type", value);
    }
    block_on(Json::<Item>::from_request(request.body(Body::from(body.to_owned())).unwrap(), &()))
}

#[test]
fn json_bodies_parse_or_fail_as_json_errors() {
    assert_eq!(post(r#"{"name":"a"}"#, Some("application/json")).unwrap().0, Item { name: "a".into() });
    for (body, content_type) in
        [(r#"{"name":1}"#, Some("application/json")), ("{", Some("application/json")), ("{}", None)]
    {
        let err = post(body, content_type).unwrap_err();
        assert!(matches!(err.0, Error::BadRequest(_)), "{body}: {err:?}");
    }
}

#[test]
fn json_and_created_responses() {
    let (status, body) = json_of(Json(Item { name: "a".into() }).into_response());
    assert_eq!((status, body), (200, serde_json::json!({"name": "a"})));
    let (status, body) = json_of(Created(Item { name: "b".into() }).into_response());
    assert_eq!((status, body), (201, serde_json::json!({"name": "b"})));
}

fn page(query: &str) -> Result<Page, ApiError> {
    let (mut parts, ()) = HttpRequest::builder().uri(format!("/?{query}")).body(()).unwrap().into_parts();
    block_on(Page::from_request_parts(&mut parts, &()))
}

#[test]
fn pages_default_and_stay_within_bounds() {
    assert_eq!(page("").unwrap(), Page { limit: 50, offset: 0 });
    assert_eq!(page("limit=100&offset=20").unwrap(), Page { limit: 100, offset: 20 });
    for bad in ["limit=0", "limit=101", "offset=-1", "limit=many"] {
        assert!(page(bad).is_err(), "{bad}");
    }
}
