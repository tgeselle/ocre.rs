use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use axum::response::IntoResponse;

use super::*;
use crate::support::body_text;

#[test]
fn steps_run_only_when_the_body_is_read_and_stop_at_none() {
    let calls = Arc::new(AtomicU32::new(0));
    let counted = calls.clone();
    let response = stream(0, move |n: u32| {
        counted.fetch_add(1, Ordering::SeqCst);
        async move { (n < 2).then(|| (Event::default().event("tick").data(format!("{n}\nline")), n + 1)) }
    })
    .into_response();
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no step before the client reads");
    assert_eq!(response.headers()["cache-control"], "no-cache");
    assert_eq!(body_text(response), "event: tick\ndata: 0\ndata: line\n\nevent: tick\ndata: 1\ndata: line\n\n");
    assert_eq!(calls.load(Ordering::SeqCst), 3, "two events, then the call that ended the stream");
}
