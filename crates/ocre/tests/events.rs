use serde_json::json;

use super::*;

struct Orders;

impl Subscriber for Orders {
    fn name(&self) -> &'static str {
        "orders-test"
    }

    fn emit(&self, event: &Event, vars: &dyn Fn(&str) -> Option<String>) -> Option<Delivery> {
        if !event.name.starts_with("order.") {
            return None;
        }
        Some(Delivery { url: vars("ORDERS_URL")?, headers: vec![], body: event.payload.len().to_string() })
    }
}

#[test]
fn events_carry_their_payload_tags_and_context() {
    let events = Events::new(crate::log::Logger::new().with("request_id", "r1"));
    events.set_context("tenant", "acme");
    events.notify("order.placed", json!({ "order_id": 42 }));
    events.tagged("step", "payment").notify("order.paid", 42);
    let taken = events.take();
    assert_eq!(taken.len(), 2);
    assert_eq!((taken[0].payload["order_id"].clone(), taken[0].context["tenant"].clone()), (json!(42), json!("acme")));
    assert!(taken[0].tags.is_empty() && taken[0].timestamp > 0);
    assert_eq!((taken[1].payload["value"].clone(), taken[1].tags["step"].clone()), (json!(42), json!("payment")));
    assert!(events.take().is_empty(), "taken once");
}

#[test]
fn subscribers_turn_events_into_deliveries() {
    subscribe(Orders);
    subscribe(Orders); // the same name replaces the first
    let events = Events::default();
    events.notify("order.placed", json!({ "a": 1, "b": 2 }));
    events.notify("user.signed_up", json!({}));
    let taken = events.take();
    let url = |name: &str| (name == "ORDERS_URL").then(|| "https://example.test/events".to_owned());
    let sent: Vec<_> = deliveries(&taken, &url).into_iter().filter(|(name, _)| *name == "orders-test").collect();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].1.url.as_str(), sent[0].1.body.as_str()), ("https://example.test/events", "2"));
    assert!(deliveries(&taken, &|_| None).iter().all(|(name, _)| *name != "orders-test"), "no URL, nothing sent");
}
