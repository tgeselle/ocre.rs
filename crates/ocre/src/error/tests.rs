use super::*;

#[test]
fn public_form_hides_internal_messages() {
    assert_eq!(Error::NotFound.into_public(), (StatusCode::NOT_FOUND, "Not found".to_owned()));
    assert_eq!(Error::bad_request("x").into_public(), (StatusCode::BAD_REQUEST, "x".to_owned()));
    let (status, message) = Error::internal("password=hunter2").into_public();
    assert_eq!((status, message.as_str()), (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"));
}

#[test]
fn display_describes_each_variant() {
    assert_eq!(Error::NotFound.to_string(), "not found");
    assert_eq!(Error::bad_request("x").to_string(), "bad request: x");
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
