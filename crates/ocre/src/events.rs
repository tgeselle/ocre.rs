//! Structured events (Rails 8.1's `Rails.event`): named facts about what
//! the app did, with a payload, for analytics, audit trails or a data
//! warehouse.
//!
//! Every [`Ctx`](crate::Ctx) has an [`Events`] notifier,
//! [`Ctx::events`](crate::Ctx::events). An event is logged at once (an
//! `info` line `event <name> <payload>` carrying the request's log fields,
//! so Workers Logs can query it) and handed to each [`Subscriber`] the app
//! registered with [`subscribe`].
//!
//! ```no_run
//! use axum::extract::State;
//! use ocre::{Ctx, Result};
//! use serde_json::json;
//!
//! async fn checkout(State(ctx): State<Ctx>) -> Result<&'static str> {
//!     ctx.events().set_context("tenant", "acme"); // on every event of this request
//!     ctx.events().notify("order.placed", json!({ "order_id": 42, "total": "19.99" }));
//!     // Tags group events of one part of the code (Rails' `Rails.event.tagged`).
//!     ctx.events().tagged("step", "payment").notify("payment.captured", json!({ "order_id": 42 }));
//!     Ok("OK")
//! }
//! # let _ = checkout;
//! ```
//!
//! # Subscribers
//!
//! Register them once per Worker instance, in the `start` event of
//! `src/lib.rs`. A subscriber turns an [`Event`] into the HTTP request to
//! send ([`Delivery`]); Ocre sends it with `fetch` after the handler, like
//! error reports.
//!
//! # Free plan
//!
//! Logging costs nothing. Each delivery is one subrequest (50 per request
//! on the free plan): batch on the receiving side, or keep events in the
//! logs only.

use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::{Map, Value};

pub use crate::errors::Delivery;
use crate::log::Logger;

/// A structured event, from [`Events::notify`].
///
/// # Examples
///
/// ```
/// let events = ocre::events::Events::default();
/// events.notify("user.signed_up", serde_json::json!({ "user_id": 7 }));
/// let event = &events.take()[0];
/// assert_eq!((event.name.as_str(), &event.payload["user_id"]), ("user.signed_up", &serde_json::json!(7)));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// What happened, e.g. `order.placed`.
    pub name: String,
    /// Its data; a payload that is not a JSON object is kept under `value`.
    pub payload: Map<String, Value>,
    /// Tags of the [`Events`] that notified it ([`Events::tagged`]).
    pub tags: Map<String, Value>,
    /// The request's or job's context ([`Events::set_context`]).
    pub context: Map<String, Value>,
    /// When it happened, in Unix seconds.
    pub timestamp: i64,
}

/// A destination for events (Rails' event subscribers): turns an [`Event`] into the request to send.
///
/// `vars` looks up a Worker variable or secret by name. Return `None` to
/// send nothing, e.g. for events it ignores.
///
/// # Examples
///
/// ```
/// use ocre::events::{Delivery, Event, Subscriber};
///
/// /// Posts every `order.*` event to an analytics endpoint.
/// struct Analytics;
///
/// impl Subscriber for Analytics {
///     fn name(&self) -> &'static str {
///         "analytics"
///     }
///
///     fn emit(&self, event: &Event, vars: &dyn Fn(&str) -> Option<String>) -> Option<Delivery> {
///         if !event.name.starts_with("order.") {
///             return None;
///         }
///         let body = serde_json::json!({ "event": event.name, "properties": event.payload }).to_string();
///         Some(Delivery { url: vars("ANALYTICS_URL")?, headers: vec![("content-type".into(), "application/json".into())], body })
///     }
/// }
///
/// ocre::events::subscribe(Analytics);
/// ```
pub trait Subscriber: Send + Sync {
    /// A short name, for failure logs: `analytics`.
    fn name(&self) -> &'static str;

    /// The request to send for `event`, if any.
    fn emit(&self, event: &Event, vars: &dyn Fn(&str) -> Option<String>) -> Option<Delivery>;
}

static SUBSCRIBERS: Mutex<Vec<Arc<dyn Subscriber>>> = Mutex::new(Vec::new());

/// Registers a subscriber for every event of this Worker instance (Rails' `Rails.event.subscribe`).
///
/// Call it from the `start` event, which runs once per instance; a second
/// subscriber with the same [`name`](Subscriber::name) replaces the first.
///
/// # Examples
///
/// ```
/// # struct Audit;
/// # impl ocre::events::Subscriber for Audit {
/// #     fn name(&self) -> &'static str { "audit" }
/// #     fn emit(&self, _: &ocre::events::Event, _: &dyn Fn(&str) -> Option<String>) -> Option<ocre::events::Delivery> { None }
/// # }
/// ocre::events::subscribe(Audit);
/// ```
pub fn subscribe(subscriber: impl Subscriber + 'static) {
    let mut subscribers = SUBSCRIBERS.lock().unwrap_or_else(PoisonError::into_inner);
    subscribers.retain(|existing| existing.name() != subscriber.name());
    subscribers.push(Arc::new(subscriber));
}

/// The deliveries of `events` for every registered subscriber, with the subscriber's name.
pub(crate) fn deliveries(events: &[Event], vars: &dyn Fn(&str) -> Option<String>) -> Vec<(&'static str, Delivery)> {
    let subscribers = SUBSCRIBERS.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let mut out = Vec::new();
    for event in events {
        for subscriber in &subscribers {
            if let Some(delivery) = subscriber.emit(event, vars) {
                out.push((subscriber.name(), delivery));
            }
        }
    }
    out
}

#[derive(Debug, Default)]
struct State {
    context: Map<String, Value>,
    pending: Vec<Event>,
}

/// The event notifier of one request, job batch or cron run: [`Ctx::events`](crate::Ctx::events).
///
/// Cheap to clone; clones share their context and pending events.
/// `Events::default()` builds one for unit tests, whose events are read back
/// with [`take`](Self::take).
///
/// # Examples
///
/// ```
/// let events = ocre::events::Events::default();
/// events.notify("cache.miss", "posts/12");
/// assert_eq!(events.take()[0].payload["value"], "posts/12");
/// ```
#[derive(Debug, Clone, Default)]
pub struct Events {
    state: Arc<Mutex<State>>,
    tags: Map<String, Value>,
    log: Logger,
}

impl Events {
    /// A notifier whose log lines carry the fields of `log` (the request id...).
    pub(crate) fn new(log: Logger) -> Self {
        Self { state: Arc::default(), tags: Map::new(), log }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records an event (Rails' `Rails.event.notify`): logs it and queues it for the subscribers.
    ///
    /// # Examples
    ///
    /// ```
    /// let events = ocre::events::Events::default();
    /// events.notify("post.published", serde_json::json!({ "post_id": 3 }));
    /// assert_eq!(events.take()[0].payload["post_id"], 3);
    /// ```
    pub fn notify(&self, name: &str, payload: impl Serialize) {
        self.notify_value(name, serde_json::to_value(payload).unwrap_or(Value::Null));
    }

    fn notify_value(&self, name: &str, payload: Value) {
        let payload = match payload {
            Value::Object(map) => map,
            value => Map::from_iter([("value".to_owned(), value)]),
        };
        let mut log = self.log.clone();
        for (key, value) in &self.tags {
            log = log.with(key, value);
        }
        log.info(format_args!("event {name} {}", Value::Object(payload.clone())));
        let mut state = self.lock();
        let context = state.context.clone();
        let event = Event { name: name.to_owned(), payload, tags: self.tags.clone(), context, timestamp: crate::now() };
        state.pending.push(event);
    }

    /// This notifier with a tag added to its events (Rails' `Rails.event.tagged`).
    ///
    /// # Examples
    ///
    /// ```
    /// let events = ocre::events::Events::default();
    /// events.tagged("importer", "csv").notify("row.skipped", serde_json::json!({ "line": 12 }));
    /// assert_eq!(events.take()[0].tags["importer"], "csv");
    /// ```
    #[must_use]
    pub fn tagged(&self, key: &str, value: impl Serialize) -> Self {
        let mut tagged = self.clone();
        tagged.tags.insert(key.to_owned(), serde_json::to_value(value).unwrap_or(Value::Null));
        tagged
    }

    /// Adds context to every later event of this request or job (Rails' `Rails.event.set_context`).
    ///
    /// # Examples
    ///
    /// ```
    /// let events = ocre::events::Events::default();
    /// events.set_context("user_id", 7);
    /// events.notify("search", serde_json::json!({ "query": "rust" }));
    /// assert_eq!(events.take()[0].context["user_id"], 7);
    /// ```
    pub fn set_context(&self, key: &str, value: impl Serialize) {
        self.lock().context.insert(key.to_owned(), serde_json::to_value(value).unwrap_or(Value::Null));
    }

    /// Takes the events not yet sent to the subscribers, oldest first.
    ///
    /// # Examples
    ///
    /// ```
    /// let events = ocre::events::Events::default();
    /// events.notify("a", ());
    /// assert_eq!(events.take().len(), 1);
    /// assert!(events.take().is_empty());
    /// ```
    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut self.lock().pending)
    }
}

#[cfg(test)]
#[path = "../tests/events.rs"]
mod tests;
