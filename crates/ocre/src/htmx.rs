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
