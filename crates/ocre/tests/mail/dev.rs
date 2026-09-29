use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower_service::Service;

use super::*;
use crate::{
    Error,
    mail::Outgoing,
    support::{block_on, body_text},
};

fn welcome() -> Result<Email> {
    Ok(Email::new("ada@example.com", "Welcome <3", "Hello Ada")
        .html("<p>Hi</p><img src=\"cid:logo\">")
        .cc("team@example.com")
        .bcc("archive@example.com")
        .reply_to("help@example.com")
        .header("X-Campaign", "spring")
        .attach("terms.txt", "text/plain", b"terms".to_vec())
        .inline("logo", "logo.png", "image/png", vec![1, 2, 3]))
}

static PREVIEWS: &[Preview] =
    &[Preview::new("user/welcome", welcome), Preview::new("user/broken", || Err(Error::internal("template missing")))];

fn get(uri: &str) -> (StatusCode, String, String) {
    let mut app = dev_routes::<()>(PREVIEWS);
    let response = block_on(app.call(Request::get(uri).body(Body::empty()).unwrap())).unwrap();
    let status = response.status();
    let csp = response
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .map(|value| value.to_str().unwrap().to_owned())
        .unwrap_or_default();
    (status, csp, body_text(response))
}

#[test]
fn previews_render_without_sending() {
    let (status, csp, index) = get("/ocre/dev/mailers");
    assert_eq!(status, StatusCode::OK);
    assert!(csp.starts_with("default-src 'none'; script-src 'self'"), "{csp}");
    assert!(index.contains("<a href=\"/ocre/dev/mailers/preview/user/welcome\">user/welcome</a>"), "{index}");

    let (status, _, page) = get("/ocre/dev/mailers/preview/user/welcome");
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("<h1>Welcome &lt;3</h1>"), "escaped: {page}");
    for row in [
        "<tr><th>From</th><td>MAIL_FROM</td></tr>",
        "<tr><th>Cc</th><td>team@example.com</td></tr>",
        "<tr><th>Bcc</th><td>archive@example.com</td></tr>",
        "<tr><th>Reply-To</th><td>help@example.com</td></tr>",
        "<tr><th>X-Campaign</th><td>spring</td></tr>",
        "<p>Attachment: terms.txt (text/plain, 5 bytes)</p>",
        "<p>Inline cid:logo: logo.png (image/png, 3 bytes)</p>",
        "src=&quot;data:image/png;base64,AQID&quot;",
        "<iframe sandbox srcdoc=",
        "<pre>Hello Ada</pre>",
    ] {
        assert!(page.contains(row), "{row} in {page}");
    }

    let (status, _, page) = get("/ocre/dev/mailers/preview/user/broken");
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(page.contains("The preview failed: internal error: template missing"), "{page}");
    assert_eq!(get("/ocre/dev/mailers/preview/nope").0, StatusCode::NOT_FOUND);
}

#[test]
fn an_app_without_previews_says_how_to_add_them() {
    let mut app = dev_routes::<()>(&[]);
    let response = block_on(app.call(Request::get("/ocre/dev/mailers").body(Body::empty()).unwrap())).unwrap();
    assert!(body_text(response).contains("<code>ocre g mailer</code> adds them"));
}

#[test]
fn logged_emails_are_captured_for_the_pages_and_tests() {
    // One test touches the shared capture, so parallel tests cannot race on it.
    let (_, _, index) = get("/ocre/dev/mailers");
    assert!(index.contains("Nothing sent since this Worker started."), "{index}");

    let plain = Email::new("grace@example.com", "Captured one", "Link: https://example.com/magic/abc");
    let from = Some("Shop <noreply@example.com>".to_owned());
    for _ in 0..(KEEP + 1) {
        capture(Outgoing::new(from.clone(), plain.clone()).unwrap());
    }
    let (_, _, index) = get("/ocre/dev/mailers");
    assert!(index.contains("Captured one</a> to grace@example.com</li>"), "{index}");
    let list: serde_json::Value = serde_json::from_str(&get("/ocre/dev/mailers/sent.json").2).unwrap();
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), KEEP, "only the last {KEEP} are kept");
    assert_eq!(list[0]["id"], 2, "the oldest was dropped");
    let last = list.last().unwrap();
    assert_eq!(last["email"]["text"], "Link: https://example.com/magic/abc");
    assert_eq!(last["from"], "Shop <noreply@example.com>");

    let id = last["id"].as_u64().unwrap();
    let (status, _, page) = get(&format!("/ocre/dev/mailers/sent/{id}"));
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("<tr><th>From</th><td>Shop &lt;noreply@example.com&gt;</td></tr>"), "{page}");
    assert!(!page.contains("<iframe"), "text-only email");
    assert_eq!(get("/ocre/dev/mailers/sent/1").0, StatusCode::NOT_FOUND);
}

#[test]
fn the_mailbox_form_posts_to_wranglers_local_endpoint() {
    let (status, csp, page) = get("/ocre/dev/mailbox");
    assert_eq!(status, StatusCode::OK);
    assert!(csp.contains("connect-src 'self'"), "{csp}");
    assert!(page.contains("<form id=\"compose\">") && page.contains("<script src=\"/ocre/dev/mailbox.js\">"));
    let (status, _, script) = get("/ocre/dev/mailbox.js");
    assert_eq!(status, StatusCode::OK);
    assert!(script.contains("fetch(`/cdn-cgi/local/email?${query}`"), "{script}");
}

#[test]
fn previews_can_be_listed_at_runtime() {
    static RUNTIME: std::sync::LazyLock<Vec<Preview>> =
        std::sync::LazyLock::new(|| vec![Preview::new("order/shipped", welcome)]);
    let mut app = dev_routes::<()>(&RUNTIME);
    let uri = "/ocre/dev/mailers/preview/order/shipped";
    let response = block_on(app.call(Request::get(uri).body(Body::empty()).unwrap())).unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_text(response).contains("<h1>Welcome &lt;3</h1>"));
}

#[test]
fn escape_covers_every_html_special_character() {
    assert_eq!(
        escape(r#"<a href="x">Tom & 'Jerry'</a>"#),
        "&lt;a href=&quot;x&quot;&gt;Tom &amp; &#39;Jerry&#39;&lt;/a&gt;"
    );
    assert_eq!(escape("plain é"), "plain é");
}
