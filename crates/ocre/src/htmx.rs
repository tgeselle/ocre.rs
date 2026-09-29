use std::convert::Infallible;

use axum::{
    extract::FromRequestParts,
    http::{HeaderValue, request::Parts},
    response::{IntoResponse, Response},
};

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

/// Response telling htmx to load another page: `200 OK` with the `HX-Redirect` header.
///
/// htmx follows a plain `303` inside the XHR and swaps the result into the
/// target, which is wrong when an inline widget (a modal, an inline edit
/// form) should end on a whole new page. `HX-Redirect` makes htmx do a full
/// browser navigation instead (Loco's `redirect_with_header_key`). Only for
/// requests where [`Htmx`] is `true`: other clients ignore the header, so
/// answer them with [`Redirect::to`](axum::response::Redirect::to). A
/// target that is not a valid header value falls back to `/`.
///
/// # Examples
///
/// ```
/// use axum::response::IntoResponse;
/// use ocre::HxRedirect;
///
/// let response = HxRedirect("/posts/7".to_owned()).into_response();
/// assert_eq!(response.status(), 200);
/// assert_eq!(response.headers()["hx-redirect"], "/posts/7");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HxRedirect(pub String);

impl IntoResponse for HxRedirect {
    fn into_response(self) -> Response {
        let target = HeaderValue::from_str(&self.0).unwrap_or(HeaderValue::from_static("/"));
        ([("hx-redirect", target)], ()).into_response()
    }
}

#[cfg(test)]
#[path = "../tests/htmx.rs"]
mod tests;
