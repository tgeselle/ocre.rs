use axum::http::Request;

use super::*;
use crate::support::block_on;

fn extract(header: Option<&str>) -> bool {
    let mut builder = Request::builder();
    if let Some(value) = header {
        builder = builder.header("HX-Request", value);
    }
    let (mut parts, ()) = builder.body(()).unwrap().into_parts();
    let Ok(Htmx(is_htmx)) = block_on(Htmx::from_request_parts(&mut parts, &()));
    is_htmx
}

#[test]
fn true_only_when_the_htmx_header_is_true() {
    assert!(extract(Some("true")));
    assert!(!extract(None));
    assert!(!extract(Some("false")));
}

#[test]
fn hx_redirect_sets_the_header_and_falls_back_to_root() {
    use axum::response::IntoResponse;
    let response = HxRedirect("/posts/1".to_owned()).into_response();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["hx-redirect"], "/posts/1");
    let response = HxRedirect("/bad\nheader".to_owned()).into_response();
    assert_eq!(response.headers()["hx-redirect"], "/");
}
