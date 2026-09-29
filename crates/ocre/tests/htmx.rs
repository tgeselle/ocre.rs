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
