use serde_json::json;

use super::*;
use crate::errors::{Options, Reporter};

#[test]
fn credentials_are_replaced() {
    assert_eq!(scrub("key sk_live_abc-12_x leaked"), "key [secret] leaked");
    assert_eq!(scrub("Authorization: Basic YWRhOnB3"), "Authorization: [secret]");
    assert_eq!(scrub("AKIAIOSFODNN7EXAMPLE used"), "[secret] used");
    assert_eq!(scrub("whsec_ alone, xwhsec_abc, Bearer, Bearer  "), "whsec_ alone, xwhsec_abc, Bearer, Bearer  ");
    assert_eq!(
        scrub("https://x.r2.dev/k?X-Amz-Credential=AK/2026/auto&X-Amz-Signature=ab12&download=1"),
        "https://x.r2.dev/k?X-Amz-Credential=[secret]&X-Amz-Signature=[secret]&download=1"
    );
    assert_eq!(scrub("form password=hunter2 sent; token="), "form password=[secret] sent; token=");
    assert_eq!(scrub("mykey=1 apikey2=3"), "mykey=1 apikey2=3", "only whole parameter names");
}

#[test]
fn emails_ids_and_tokens_are_replaced() {
    assert_eq!(scrub("user ada.l+x@example.co.uk failed"), "user [email] failed");
    assert_eq!(scrub("not@mail, @x.y, a@b"), "not@mail, @x.y, a@b");
    assert_eq!(scrub("video 0B9C2F44-1d2e-4f7a-9c3b-2a1b0c9d8e7f gone"), "video [id] gone");
    assert_eq!(scrub("0b9c2f44-1d2e-4f7a-9c3b-2a1b0c9d8e7fx"), "[secret]", "not a UUID, still a token");
    assert_eq!(scrub("0b9c2f44-1d2e-4f7a-9c3b"), "0b9c2f44-1d2e-4f7a-9c3b", "too short");
    assert_eq!(scrub("x0b9c2f44-1d2e-4f7a-9c3b-2a1b0c9d8e7f"), "[secret]");
    let digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(scrub(&format!("sha {digest}.")), "sha [secret].");
    assert_eq!(scrub(&"a".repeat(40)), "a".repeat(40), "letters only are words");
}

#[test]
fn durations_are_normalized_for_grouping() {
    assert_eq!(scrub("took 12.5 ms, 3s and 40µs, 40 items"), "took [n]ms, [n]s and [n]µs, 40 items");
    assert_eq!(scrub("v2s 12sec 1.x"), "v2s 12sec 1.x");
}

#[test]
fn reports_are_scrubbed_with_their_context() {
    let reporter = Reporter::default();
    let context = json!({"to": ["ada@example.com"], "user": {"email": "bo@example.com"}, "id": 7});
    reporter.report(&"mail to ada@example.com failed", Options::new().context("mail", context));
    let report = reporter.take().remove(0).scrubbed();
    assert_eq!(report.message, "mail to [email] failed");
    assert_eq!(report.context["mail"], json!({"to": ["[email]"], "user": {"email": "[email]"}, "id": 7}));
}
