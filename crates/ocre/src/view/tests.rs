use std::fmt;

use super::*;
use crate::test_util::body_text;

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

fn page(err: Error) -> (u16, String) {
    let response = err.into_response();
    let status = response.status().as_u16();
    (status, body_text(response))
}

#[test]
fn errors_render_as_escaped_html_pages() {
    assert_eq!(page(Error::NotFound), (404, "<h1>404</h1><p>Not found</p>".to_owned()));
    assert_eq!(
        page(Error::bad_request(r#"<b>"Tom" & 'Jerry'</b>"#)),
        (400, "<h1>400</h1><p>&lt;b&gt;&quot;Tom&quot; &amp; &#39;Jerry&#39;&lt;/b&gt;</p>".to_owned())
    );
    assert_eq!(page(Error::internal("secret")), (500, "<h1>500</h1><p>Internal server error</p>".to_owned()));
}
