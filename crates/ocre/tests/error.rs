use super::*;

fn public(err: Error) -> (StatusCode, String, Vec<FieldError>) {
    let Public { status, message, fields, internal } = err.into_public();
    assert_eq!(internal.is_some(), status == StatusCode::INTERNAL_SERVER_ERROR);
    (status, message, fields)
}

#[test]
fn public_form_hides_internal_messages() {
    assert_eq!(public(Error::NotFound), (StatusCode::NOT_FOUND, "Not found".to_owned(), vec![]));
    assert_eq!(public(Error::bad_request("x")), (StatusCode::BAD_REQUEST, "x".to_owned(), vec![]));
    assert_eq!(public(Error::Unauthorized), (StatusCode::UNAUTHORIZED, "Unauthorized".to_owned(), vec![]));
    assert_eq!(public(Error::Forbidden), (StatusCode::FORBIDDEN, "Forbidden".to_owned(), vec![]));
    assert_eq!(public(Error::PayloadTooLarge("big".into())), (StatusCode::PAYLOAD_TOO_LARGE, "big".to_owned(), vec![]));
    assert_eq!(
        public(Error::TooManyRequests),
        (StatusCode::TOO_MANY_REQUESTS, "Too many requests. Try again later.".to_owned(), vec![])
    );
    let (status, message, _) = public(Error::internal("password=hunter2"));
    assert_eq!((status, message.as_str()), (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"));
    assert_eq!(Error::internal("D1 down").into_public().internal.as_deref(), Some("D1 down"));
}

#[test]
fn only_internal_errors_mark_their_response_for_reporting() {
    let mut response = axum::response::Response::default();
    mark(None, &mut response);
    assert!(response.extensions().get::<InternalError>().is_none());
    mark(Some("D1 down".into()), &mut response);
    assert_eq!(response.extensions().get::<InternalError>().unwrap().0, "D1 down");
    log_internal("logged as an error line");
}

#[test]
fn invalid_is_a_422_with_field_errors() {
    let fields = vec![FieldError::new("title", "can't be blank")];
    assert_eq!(
        public(Error::Invalid(fields.clone())),
        (StatusCode::UNPROCESSABLE_ENTITY, "Validation failed".to_owned(), fields)
    );
}

#[test]
fn display_describes_each_variant() {
    assert_eq!(Error::NotFound.to_string(), "not found");
    assert_eq!(Error::bad_request("x").to_string(), "bad request: x");
    assert_eq!(Error::Unauthorized.to_string(), "unauthorized");
    assert_eq!(Error::Forbidden.to_string(), "forbidden");
    assert_eq!(Error::PayloadTooLarge("big".into()).to_string(), "payload too large: big");
    assert_eq!(Error::TooManyRequests.to_string(), "too many requests");
    let invalid =
        Error::Invalid(vec![FieldError::new("title", "can't be blank"), FieldError::new("pages", "is invalid")]);
    assert_eq!(invalid.to_string(), "invalid: Title can't be blank, Pages is invalid");
    assert_eq!(Error::internal("y").to_string(), "internal error: y");
}

#[test]
fn runtime_errors_become_internal() {
    let err: Error = worker::Error::RustError("boom".into()).into();
    assert_eq!(err.to_string(), "internal error: boom");
}

#[test]
fn or_404_maps_none_to_not_found() {
    assert_eq!(Some(3).or_404().unwrap(), 3);
    assert!(matches!(None::<i32>.or_404(), Err(Error::NotFound)));
}
