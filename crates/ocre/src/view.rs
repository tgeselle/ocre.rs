//! HTML rendering (feature `html`): askama templates and HTML error pages.

use askama::Template;
use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
};

use crate::{Error, FieldError, Result};

/// Renders an askama template (compiled at build time) into an HTML response.
///
/// askama escapes interpolated values; never mark user input `|safe`.
/// Rendering is plain Rust string building: no binding call, a little CPU.
/// Requires the `html` feature.
///
/// # Errors
///
/// [`Error::Internal`] (500, logged) when the template fails at run time,
/// e.g. a `Display` implementation or a filter returns an error.
///
/// # Examples
///
/// ```
/// use askama::Template;
///
/// #[derive(Template)]
/// #[template(source = "<h1>{{ title }}</h1>", ext = "html")]
/// struct Show<'a> {
///     title: &'a str,
/// }
///
/// let html = ocre::render(&Show { title: "Tom & Jerry" }).unwrap();
/// assert_eq!(html.0, "<h1>Tom &#38; Jerry</h1>");
/// ```
pub fn render<T: Template>(template: &T) -> Result<Html<String>> {
    Ok(Html(template.render()?))
}

impl From<askama::Error> for Error {
    fn from(err: askama::Error) -> Self {
        Self::Internal(format!("template rendering failed: {err}"))
    }
}

/// HTML error page. JSON endpoints return [`ApiError`](crate::ApiError) instead.
///
/// The page is a bare `<h1>404</h1><p>Not found</p>`; the response carries
/// an [`ErrorPage`] extension so [`error_page`] can render the app's own
/// template instead.
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let public = self.into_public();
        let page = ErrorPage { status: public.status, message: public.message, fields: public.fields };
        let mut response = (page.status, Html(page.default_html())).into_response();
        response.extensions_mut().insert(page);
        crate::error::mark(public.internal, &mut response);
        response
    }
}

/// What an HTML error page may show: the status, a message safe for users, and validation errors.
///
/// Built for every [`Error`] rendered as HTML (the message of a 500 is
/// always `Internal server error`; details only go to the logs), and for
/// bodyless or plain-text error responses such as axum's rejections, with
/// the status's reason phrase as message. Templates given to [`error_page`]
/// receive it.
///
/// # Examples
///
/// ```
/// use axum::{http::StatusCode, response::IntoResponse};
/// use ocre::{Error, ErrorPage};
///
/// let response = Error::NotFound.into_response();
/// let page = response.extensions().get::<ErrorPage>().unwrap();
/// assert_eq!((page.status, page.message.as_str()), (StatusCode::NOT_FOUND, "Not found"));
/// ```
#[derive(Debug, Clone)]
pub struct ErrorPage {
    /// HTTP status of the response, e.g. `404`: `{{ error.status.as_u16() }}` in a template.
    pub status: StatusCode,
    /// Message for the user: `Not found`, `Validation failed`, a bad request's own message...
    pub message: String,
    /// Field errors of a 422 (`{{ field.full_message() }}`), empty otherwise.
    pub fields: Vec<FieldError>,
}

impl ErrorPage {
    fn default_html(&self) -> String {
        let mut page = format!("<h1>{}</h1><p>{}</p>", self.status.as_u16(), escape(&self.message));
        if !self.fields.is_empty() {
            page.push_str("<ul>");
            for field in &self.fields {
                page.push_str(&format!("<li>{}</li>", escape(&field.full_message())));
            }
            page.push_str("</ul>");
        }
        page
    }
}

/// Renders error responses with the app's own template (Rails' `public/404.html` and `500.html`).
///
/// Call it from an [`axum::middleware::map_response`] layer on the app's
/// router, so every HTML error gets the layout: [`Error`]s returned by
/// handlers (their [`ErrorPage`]), the 404 of the router's fallback, and
/// bodyless or plain-text error statuses (axum's rejections, e.g. a path
/// that does not parse), whose message is the status's reason phrase.
/// Responses that already have an HTML or JSON body, and success
/// responses, go out unchanged. The status and headers are kept. When
/// `render` fails, the error is logged and the plain page is sent. Costs a
/// template render per error response, no binding call.
///
/// `ocre new` generates this in `src/lib.rs`, with `templates/error.html`:
///
/// ```no_run
/// use askama::Template;
/// use axum::{Router, middleware::map_response, response::{Html, Response}};
/// use ocre::{Ctx, Error, ErrorPage, Result, render};
///
/// fn routes() -> Router<Ctx> {
///     Router::new()
///         // ...routes...
///         .fallback(not_found)
///         .layer(map_response(error_page))
/// }
///
/// async fn not_found() -> Error {
///     Error::NotFound
/// }
///
/// #[derive(Template)]
/// #[template(source = "<h1>{{ error.message }}</h1>", ext = "html")]
/// struct ErrorView<'a> {
///     error: &'a ErrorPage,
/// }
///
/// async fn error_page(response: Response) -> Response {
///     ocre::error_page(response, |error| render(&ErrorView { error }))
/// }
/// # let _ = routes;
/// ```
///
/// # Examples
///
/// ```
/// use axum::{http::StatusCode, response::{Html, IntoResponse}};
/// use ocre::Error;
///
/// let custom = |error: &ocre::ErrorPage| Ok(Html(format!("<main>{}</main>", error.message)));
/// let response = ocre::error_page(Error::NotFound.into_response(), custom);
/// assert_eq!(response.status(), StatusCode::NOT_FOUND);
///
/// let rejected = (StatusCode::BAD_REQUEST, "Invalid URL").into_response();
/// assert_eq!(ocre::error_page(rejected, custom).status(), StatusCode::BAD_REQUEST);
/// ```
pub fn error_page(response: Response, render: impl FnOnce(&ErrorPage) -> Result<Html<String>>) -> Response {
    let status = response.status();
    let page = match response.extensions().get::<ErrorPage>() {
        Some(page) => page.clone(),
        None if (status.is_client_error() || status.is_server_error()) && !has_rich_body(&response) => {
            let message = status.canonical_reason().unwrap_or("Error").to_owned();
            ErrorPage { status, message, fields: vec![] }
        }
        None => return response,
    };
    let html = render(&page).map_or_else(
        |err| {
            crate::error::log_internal(&format!("error page template failed: {err}"));
            page.default_html()
        },
        |Html(html)| html,
    );
    let (mut parts, _) = response.into_parts();
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    parts.extensions.insert(page);
    Response::from_parts(parts, Body::from(html))
}

/// Whether the body is HTML or JSON already (a page, an API error): only
/// bodyless and plain-text errors get the error page.
fn has_rich_body(response: &Response) -> bool {
    let content_type = response.headers().get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok());
    content_type.is_some_and(|value| !value.starts_with("text/plain"))
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
#[path = "../tests/view.rs"]
mod tests;
