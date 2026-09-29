use askama::Template;
use axum::response::Html;

use crate::Result;

/// Renders an askama template (compiled at build time) into an HTML response.
pub fn render<T: Template>(template: &T) -> Result<Html<String>> {
    Ok(Html(template.render()?))
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[derive(Template)]
    #[template(source = "<p>{{ name }}</p>", ext = "html")]
    struct Greeting<'a> {
        name: &'a str,
    }

    #[test]
    fn renders_html_with_escaped_values() {
        let Html(html) = render(&Greeting { name: "<Ocre>" }).unwrap();
        assert_eq!(html, "<p>&#60;Ocre&#62;</p>");
    }

    struct Unprintable;

    impl fmt::Display for Unprintable {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    #[derive(Template)]
    #[template(source = "{{ value }}", ext = "html")]
    struct Broken {
        value: Unprintable,
    }

    #[test]
    fn rendering_failure_is_an_internal_error() {
        let err = render(&Broken { value: Unprintable }).unwrap_err();
        assert!(err.to_string().starts_with("internal error: template rendering failed"), "{err}");
    }
}
