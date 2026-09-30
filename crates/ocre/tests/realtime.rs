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
    assert!(
        message.contains("CHANNELS: bindings.durableObject({ worker: \"<app>\", exportName: \"OcreChannel\" }),"),
        "{message}"
    );
    assert!(message.contains("OcreChannel: exports.durableObject({ storage: \"sqlite\" }),"), "{message}");
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

#[test]
fn subscribers_travel_in_a_header_and_default_to_listeners() {
    let upgrade = extract(&[("upgrade", "websocket")]).unwrap();
    assert_eq!(upgrade.subscriber, Subscriber::default());
    let upgrade = upgrade.identified_by("Zoé \"7\"").rebroadcast();
    let header = upgrade.subscriber_header().unwrap();
    assert!(header.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{header}");
    assert_eq!(
        Subscriber::from_header(Some(&header)),
        Subscriber { identity: Some("Zoé \"7\"".to_owned()), rebroadcast: true }
    );
    for unreadable in [None, Some("%%%"), Some("bm90IGpzb24")] {
        assert_eq!(Subscriber::from_header(unreadable), Subscriber::default(), "{unreadable:?}");
    }
    let err = extract(&[("upgrade", "websocket")])
        .unwrap()
        .identified_by("x".repeat(MAX_IDENTITY_LEN + 1))
        .subscriber_header()
        .unwrap_err();
    assert!(err.to_string().contains("more than 256"), "{err}");
    let fits = extract(&[("upgrade", "websocket")]).unwrap().identified_by("x".repeat(MAX_IDENTITY_LEN));
    assert!(fits.subscriber_header().is_ok());
}

#[test]
fn only_publishers_are_relayed_as_json() {
    let listener = Subscriber { identity: Some("ada".to_owned()), rebroadcast: false };
    assert_eq!(listener.relay("hi"), None);
    let publisher = Subscriber { identity: Some("ada".to_owned()), rebroadcast: true };
    assert_eq!(publisher.relay(r#"{"text":"hi"}"#).unwrap(), r#"{"data":{"text":"hi"},"from":"ada"}"#);
    assert_eq!(publisher.relay("<b>hi</b>").unwrap(), r#"{"data":"<b>hi</b>","from":"ada"}"#);
    let anonymous = Subscriber { identity: None, rebroadcast: true };
    assert_eq!(anonymous.relay("1").unwrap(), r#"{"data":1,"from":null}"#);
    assert!(publisher.relay(&"x".repeat(MAX_REBROADCAST_BYTES)).is_some());
    assert_eq!(publisher.relay(&"x".repeat(MAX_REBROADCAST_BYTES + 1)), None);
}

#[test]
fn dev_routes_list_recent_broadcasts() {
    use tower_service::Service;

    for n in 0..55 {
        record("posts", &format!("<p>{n}</p>"));
    }
    let mut app = dev_routes::<()>();
    let request = Request::get("/ocre/dev/realtime/sent.json").body(axum::body::Body::empty()).unwrap();
    let response = block_on(app.call(request)).unwrap();
    let sent: Vec<serde_json::Value> = serde_json::from_str(&body_text(response)).unwrap();
    assert_eq!(sent.len(), 50);
    let last = sent.last().unwrap();
    assert_eq!((last["channel"].as_str(), last["message"].as_str()), (Some("posts"), Some("<p>54</p>")));
    assert!(sent.windows(2).all(|pair| pair[0]["id"].as_u64() < pair[1]["id"].as_u64()));
}

#[test]
fn identified_publishers_announce_presence() {
    let publisher = Subscriber { identity: Some("ada".to_owned()), rebroadcast: true };
    assert_eq!(publisher.presence("joined").unwrap(), r#"{"event":"joined","from":"ada"}"#);
    assert_eq!(Subscriber { identity: Some("ada".to_owned()), rebroadcast: false }.presence("left"), None);
    assert_eq!(Subscriber { identity: None, rebroadcast: true }.presence("left"), None);
}
