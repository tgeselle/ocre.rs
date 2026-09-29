use std::fmt;

use super::*;
use crate::support::body_text;

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
    let invalid = Error::Invalid(vec![crate::FieldError::new("title", "can't be <blank>")]);
    assert_eq!(
        page(invalid),
        (422, "<h1>422</h1><p>Validation failed</p><ul><li>Title can&#39;t be &lt;blank&gt;</li></ul>".to_owned())
    );
}

fn custom(error: &ErrorPage) -> Result<Html<String>> {
    Ok(Html(format!("<main>{} {} {}</main>", error.status.as_u16(), error.message, error.fields.len())))
}

#[test]
fn error_page_renders_errors_with_the_app_template() {
    let invalid = Error::Invalid(vec![crate::FieldError::new("title", "can't be blank")]);
    let response = error_page(invalid.into_response(), custom);
    assert_eq!(response.status(), 422);
    assert_eq!(response.headers()["content-type"], "text/html; charset=utf-8");
    assert!(response.extensions().get::<ErrorPage>().is_some());
    assert_eq!(body_text(response), "<main>422 Validation failed 1</main>");
}

#[test]
fn error_page_covers_bodyless_and_plain_text_errors_only() {
    let empty = StatusCode::NOT_FOUND.into_response();
    assert_eq!(body_text(error_page(empty, custom)), "<main>404 Not Found 0</main>");
    let rejection = (StatusCode::BAD_REQUEST, [("x-kept", "1")], "Invalid URL").into_response();
    let response = error_page(rejection, custom);
    assert_eq!(response.headers()["x-kept"], "1");
    assert_eq!(body_text(response), "<main>400 Bad Request 0</main>");
    let unknown = StatusCode::from_u16(599).unwrap().into_response();
    assert_eq!(body_text(error_page(unknown, custom)), "<main>599 Error 0</main>");
    // Pages, JSON errors and successes are left alone.
    let html = (StatusCode::UNPROCESSABLE_ENTITY, Html("<form>")).into_response();
    assert_eq!(body_text(error_page(html, custom)), "<form>");
    let json = crate::ApiError(Error::NotFound).into_response();
    assert!(body_text(error_page(json, custom)).starts_with("{"));
    assert_eq!(body_text(error_page("OK".into_response(), custom)), "OK");
}

#[test]
fn error_page_falls_back_to_the_plain_page_when_the_template_fails() {
    let failing = |_: &ErrorPage| Err(Error::internal("broken template"));
    let response = error_page(Error::Forbidden.into_response(), failing);
    assert_eq!(response.status(), 403);
    assert_eq!(body_text(response), "<h1>403</h1><p>Forbidden</p>");
}
