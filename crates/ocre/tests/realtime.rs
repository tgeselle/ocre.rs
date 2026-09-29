use axum::http::Request;

use super::*;
use crate::support::{block_on, body_text};

#[test]
fn channel_names_are_url_safe() {
    for valid in ["posts", "post:12", "room_1.lobby-a", &"a".repeat(MAX_CHANNEL_LEN)] {
        assert_eq!(channel_error(valid), None, "{valid}");
    }
    for invalid in ["", "a b", "posts/1", "café", "<x>", &"a".repeat(MAX_CHANNEL_LEN + 1)] {
        let message = channel_error(invalid).unwrap();
        assert!(message.starts_with(&format!("invalid channel name `{invalid}`")), "{message}");
    }
}

#[test]
fn missing_binding_names_the_fix() {
    let Error::Internal(message) = missing_binding("Binding `CHANNELS` is undefined.") else { panic!() };
    assert!(message.contains("[[durable_objects.bindings]] name = \"CHANNELS\", class_name = \"OcreChannel\""));
    assert!(message.contains("new_sqlite_classes = [\"OcreChannel\"]"), "{message}");
}

#[test]
fn close_codes_the_server_may_send() {
    assert_eq!(close_code(1001), 1001);
    assert_eq!(close_code(4000), 4000);
    for reserved in [1005, 1006, 1015, 999, 5000, 2000] {
        assert_eq!(close_code(reserved), 1000, "{reserved}");
    }
}

fn extract(headers: &[(&str, &str)]) -> Result<WebSocketUpgrade, Rejection> {
    let mut request = Request::builder().uri("/realtime/posts");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let (mut parts, ()) = request.body(()).unwrap().into_parts();
    block_on(WebSocketUpgrade::from_request_parts(&mut parts, &()))
}

#[cfg(feature = "html")]
#[test]
fn upgrade_extractor_needs_a_websocket_handshake() {
    assert!(extract(&[("upgrade", "websocket")]).is_ok());
    assert!(extract(&[("upgrade", "WebSocket")]).is_ok());
    let rejected = extract(&[]).unwrap_err();
    assert!(matches!(&rejected, Error::BadRequest(message) if message.starts_with("Expected a WebSocket connection")));
    let response = axum::response::IntoResponse::into_response(rejected);
    assert_eq!(response.status(), 400);
    assert!(body_text(response).contains("Upgrade: websocket"));
}

#[test]
fn swaps_wrap_fragments_where_the_browser_parses_them() {
    assert_eq!(
        prepend("posts", "<tr id=\"post_1\"><td>a</td></tr>"),
        "<tbody hx-swap-oob=\"afterbegin:#posts\"><tr id=\"post_1\"><td>a</td></tr></tbody>"
    );
    assert_eq!(append("row", " <TD>x</TD>"), "<tr hx-swap-oob=\"beforeend:#row\"> <TD>x</TD></tr>");
    assert_eq!(append("t", "<thead></thead>"), "<table hx-swap-oob=\"beforeend:#t\"><thead></thead></table>");
    assert_eq!(append("l", "<li>x</li>"), "<ul hx-swap-oob=\"beforeend:#l\"><li>x</li></ul>");
    assert_eq!(append("s", "<option>x</option>"), "<select hx-swap-oob=\"beforeend:#s\"><option>x</option></select>");
    assert_eq!(append("d", "<dd>x</dd>"), "<dl hx-swap-oob=\"beforeend:#d\"><dd>x</dd></dl>");
    assert_eq!(update("count", "3"), "<div hx-swap-oob=\"innerHTML:#count\">3</div>");
    assert_eq!(update("p", "<p>x</p>"), "<div hx-swap-oob=\"innerHTML:#p\"><p>x</p></div>");
    assert_eq!(remove("post_1"), "<div id=\"post_1\" hx-swap-oob=\"delete\"></div>");
    assert_eq!(remove("a\"&<"), "<div id=\"a&quot;&amp;&lt;\" hx-swap-oob=\"delete\"></div>", "escaped");
}
