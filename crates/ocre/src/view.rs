use askama::Template;
use axum::response::Html;

use crate::Result;

/// Renders an askama template (compiled at build time) into an HTML response.
pub fn render<T: Template>(template: &T) -> Result<Html<String>> {
    Ok(Html(template.render()?))
}
