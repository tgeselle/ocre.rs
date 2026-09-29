//! HTML rendering (feature `html`): askama templates and HTML error pages.

use askama::Template;
use axum::response::{Html, IntoResponse, Response};

use crate::{Error, Result};

/// Renders an askama template (compiled at build time) into an HTML response.
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
        let (status, message) = self.into_public();
        (status, Html(format!("<h1>{}</h1><p>{}</p>", status.as_u16(), escape(&message)))).into_response()
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
mod tests;
