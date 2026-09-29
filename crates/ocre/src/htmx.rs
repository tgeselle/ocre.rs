use std::convert::Infallible;

use axum::{extract::FromRequestParts, http::request::Parts};

/// Extractor telling whether htmx sent the request: `Htmx(true)` when it has `HX-Request: true`.
///
/// Use it to answer with an HTML fragment instead of a full page or a
/// redirect. It never rejects: any other value or a missing header is
/// `Htmx(false)`. Requires the `html` feature.
///
/// # Examples
///
/// ```no_run
/// use axum::response::{Html, IntoResponse, Redirect, Response};
/// use ocre::Htmx;
///
/// async fn like(Htmx(is_htmx): Htmx) -> Response {
///     if is_htmx {
///         Html("<button disabled>Liked</button>").into_response()
///     } else {
///         Redirect::to("/").into_response()
///     }
/// }
/// # let _ = like;
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Htmx(pub bool);

impl<S: Send + Sync> FromRequestParts<S> for Htmx {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(parts.headers.get("hx-request").is_some_and(|v| v == "true")))
    }
}

#[cfg(test)]
#[path = "../tests/htmx.rs"]
mod tests;
