//! Server-Sent Events: a response that sends events as they happen (Rails' `ActionController::Live` with `SSE`).
//!
//! A handler returns [`stream()`](fn@stream)`(state, step)`: Ocre calls `step` for each
//! event, and the Worker streams every event to the client as soon as it is
//! produced, until `step` returns `None`. The response is axum's [`Sse`]
//! (`Content-Type: text/event-stream`, `Cache-Control: no-cache`), which
//! browsers read with `new EventSource(url)` or htmx's SSE extension.
//! Pace events with [`sleep`](crate::sleep).
//!
//! Streams fit work that one request follows from start to end: the
//! progress of an import, tokens from an AI model, a countdown. To push
//! changes to many open pages (a new comment), use [`realtime`] channels,
//! which do not keep a request open per page.
//!
//! # Free plan
//!
//! - **CPU**: only the work of each `step` counts toward the 10 ms per
//!   request; waiting in [`sleep`](crate::sleep) or on I/O does not.
//! - **Duration**: a response may stream for as long as the client stays
//!   connected; when the client goes away, the stream stops at the next event.
//! - **One invocation**: the whole stream is one request. Its binding calls
//!   count toward the per-request limits (50 subrequests on the free plan,
//!   D1 queries included), so a stream cannot poll D1 every second for
//!   minutes; end it and let the client reconnect, or use [`realtime`].
//! - **Reconnects**: `EventSource` reconnects about 3 s after a stream ends,
//!   and each reconnect is a new request (100,000 a day on the free plan).
//!   Send a last event (e.g. `event: done`) on which the page calls
//!   `source.close()`, or set [`Event::retry`] to reconnect less often.
//!
//! [`realtime`]: https://docs.rs/ocre/latest/ocre/realtime/index.html
//!
//! # Examples
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use axum::{Router, routing::get};
//! use ocre::{Ctx, sse::{self, Event}};
//!
//! pub fn routes() -> Router<Ctx> {
//!     Router::new().route("/countdown", get(countdown))
//! }
//!
//! /// `data: 3`, `data: 2`, `data: 1` one second apart, then `event: done`.
//! async fn countdown() -> impl axum::response::IntoResponse {
//!     sse::stream(Some(3), |left: Option<u32>| async move {
//!         let left = left?;
//!         ocre::sleep(Duration::from_secs(1)).await;
//!         Some(match left {
//!             0 => (Event::default().event("done").data(""), None),
//!             n => (Event::default().data(n.to_string()), Some(n - 1)),
//!         })
//!     })
//! }
//! ```
//!
//! In the page: `const source = new EventSource("/countdown");
//! source.addEventListener("done", () => source.close());`.

use std::{convert::Infallible, future::Future};

pub use axum::response::sse::{Event, Sse};
use futures_util::{Stream, StreamExt as _, stream};

/// An event stream: `step(state)` gives the next event and the next state, or `None` to end the stream.
///
/// `step` runs once per event, only while the client reads the response;
/// the first call happens after the handler returned (so the response
/// headers leave at once). `state` carries what the next step needs (a
/// counter, a cursor, an id); it must be `Send` like every Ocre value. See
/// the [module documentation](self) for costs and limits.
///
/// # Examples
///
/// ```
/// use axum::response::IntoResponse;
/// use ocre::sse::{self, Event};
///
/// let response = sse::stream(1, |n: u32| async move {
///     (n <= 2).then(|| (Event::default().id(n.to_string()).data(format!("step {n}")), n + 1))
/// })
/// .into_response();
/// assert_eq!(response.headers()["content-type"], "text/event-stream");
/// let body = pollster::block_on(axum::body::to_bytes(response.into_body(), usize::MAX)).unwrap();
/// assert_eq!(body, "id: 1\ndata: step 1\n\nid: 2\ndata: step 2\n\n");
/// ```
pub fn stream<T, F, Fut>(state: T, step: F) -> Sse<impl Stream<Item = Result<Event, Infallible>> + Send + 'static>
where
    T: Send + 'static,
    F: FnMut(T) -> Fut + Send + 'static,
    Fut: Future<Output = Option<(Event, T)>> + Send + 'static,
{
    Sse::new(stream::unfold(state, step).map(Ok))
}

#[cfg(test)]
#[path = "../tests/sse.rs"]
mod tests;
