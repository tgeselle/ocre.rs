//! HTML rendering (feature `html`): askama templates and HTML error pages.

use askama::Template;
use axum::response::{Html, IntoResponse, Response};

use crate::{Error, Result};

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
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let public = self.into_public();
        let mut page = format!("<h1>{}</h1><p>{}</p>", public.status.as_u16(), escape(&public.message));
        if !public.fields.is_empty() {
            page.push_str("<ul>");
            for field in &public.fields {
                page.push_str(&format!("<li>{}</li>", escape(&field.full_message())));
            }
            page.push_str("</ul>");
        }
        (public.status, Html(page)).into_response()
    }
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
