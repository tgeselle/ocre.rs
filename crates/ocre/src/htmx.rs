use std::convert::Infallible;

use axum::{extract::FromRequestParts, http::request::Parts};

/// `Htmx(true)` when the request was sent by htmx (`HX-Request: true`).
/// Use it to answer with an HTML fragment instead of a full page or redirect.
#[derive(Debug, Clone, Copy)]
pub struct Htmx(pub bool);

impl<S: Send + Sync> FromRequestParts<S> for Htmx {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(parts.headers.get("hx-request").is_some_and(|v| v == "true")))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;

    use super::*;
    use crate::test_util::block_on;

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
}
