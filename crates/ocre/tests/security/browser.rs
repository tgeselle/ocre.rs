use axum::{Router, body::Body, http::Request as HttpRequest, routing::get};

use super::*;
use crate::support::{block_on, body_text};

const SAFARI_17_1: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_1) AppleWebKit/605.1.15 (KHTML, like Gecko) \
                           Version/17.1 Safari/605.1.15";
const SAFARI_17_2: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_2) AppleWebKit/605.1.15 (KHTML, like Gecko) \
                           Version/17.2 Safari/605.1.15";
const CHROME_IOS_119: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) AppleWebKit/605.1.15 \
                              (KHTML, like Gecko) CriOS/119.0.6045.169 Mobile/15E148 Safari/604.1";
const OPERA_105: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
                         Chrome/119.0.0.0 Safari/537.36 OPR/105.0.0.0";
const EDGE_ANDROID_121: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) \
                                Chrome/121.0.0.0 Mobile Safari/537.36 EdgA/121.0.0.0";
const EDGE_IOS_119: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) AppleWebKit/605.1.15 \
                            (KHTML, like Gecko) Version/17.0 EdgiOS/119.2151.65 Mobile/15E148 Safari/605.1.15";
const IE_11: &str = "Mozilla/5.0 (Windows NT 10.0; WOW64; Trident/7.0; rv:11.0) like Gecko";

#[test]
fn detection_prefers_the_most_specific_token() {
    assert_eq!(Browser::detect(SAFARI_17_1), Some((Browser::Safari, (17, 1))));
    assert_eq!(Browser::detect(CHROME_IOS_119), Some((Browser::Chrome, (119, 0))));
    assert_eq!(Browser::detect(OPERA_105), Some((Browser::Opera, (105, 0))));
    assert_eq!(Browser::detect(EDGE_ANDROID_121), Some((Browser::Edge, (121, 0))));
    assert_eq!(Browser::detect(EDGE_IOS_119), Some((Browser::Edge, (119, 2151))));
    assert_eq!(Browser::detect(IE_11), Some((Browser::InternetExplorer, (11, 0))));
    assert_eq!(Browser::detect("Mozilla/5.0 (X11; rv:115.0) Gecko Firefox/115"), Some((Browser::Firefox, (115, 0))));
    assert_eq!(Browser::detect("Version/17.2 without the Safari token"), None);
    assert_eq!(Browser::detect("Chrome/abc"), None, "no version number");
}

#[test]
fn modern_matches_rails_versions() {
    let modern = AllowBrowser::modern();
    for (ua, allowed) in [
        (SAFARI_17_1, false),
        (SAFARI_17_2, true),
        (CHROME_IOS_119, false),
        (OPERA_105, false),
        (EDGE_ANDROID_121, true),
        (EDGE_IOS_119, false),
        (IE_11, false),
        ("Mozilla/5.0 (X11; rv:121.0) Gecko/20100101 Firefox/121.0", true),
        ("UptimeRobot/2.0", true),
    ] {
        assert_eq!(modern.allows(Some(ua)), allowed, "{ua}");
    }
    let relaxed = AllowBrowser::default().minimum(Browser::Safari, 18, 0).minimum(Browser::Safari, 17, 0);
    assert!(relaxed.allows(Some(SAFARI_17_1)), "a later rule replaces the earlier one");
    assert!(relaxed.allows(Some(IE_11)), "browsers without a rule pass");
}

fn app(policy: AllowBrowser) -> Router {
    Router::new().route("/", get(|| async { "home" })).layer(policy)
}

fn call(app: &mut Router, user_agent: Option<&str>) -> (StatusCode, Option<String>, String) {
    let mut builder = HttpRequest::builder().uri("/");
    if let Some(user_agent) = user_agent {
        builder = builder.header("user-agent", user_agent);
    }
    let response = block_on(app.call(builder.body(Body::empty()).unwrap())).unwrap();
    let content_type = response.headers().get("content-type").map(|value| value.to_str().unwrap().to_owned());
    (response.status(), content_type, body_text(response))
}

#[test]
fn the_layer_answers_406_with_an_html_page() {
    let mut modern = app(AllowBrowser::modern());
    assert_eq!(
        call(&mut modern, Some(SAFARI_17_2)),
        (StatusCode::OK, Some("text/plain; charset=utf-8".into()), "home".into())
    );
    assert_eq!(call(&mut modern, None).0, StatusCode::OK);
    let (status, content_type, body) = call(&mut modern, Some(IE_11));
    assert_eq!((status, content_type.as_deref()), (StatusCode::NOT_ACCEPTABLE, Some("text/html; charset=utf-8")));
    assert!(body.contains("Please upgrade your browser"), "{body}");
    let mut custom = app(AllowBrowser::modern().page("<p>Upgrade</p>"));
    assert_eq!(call(&mut custom, Some(SAFARI_17_1)).2, "<p>Upgrade</p>");
}
