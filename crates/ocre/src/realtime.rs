//! Realtime updates: WebSocket channels on a Durable Object, HTML broadcasts for htmx (feature `realtime`).
//!
//! Works like Rails' Action Cable and Turbo Streams: browsers subscribe to a
//! named channel over a WebSocket, and handlers or jobs
//! [`broadcast`] HTML fragments (or JSON) to every
//! subscriber.
//!
//! ```no_run
//! use axum::{Router, extract::{Path, State}, response::Response, routing::{get, post}};
//! use ocre::{Ctx, Error, Result, realtime::{self, WebSocketUpgrade}};
//!
//! // src/realtime.rs: who may listen to which channel (`ocre g scaffold ... --realtime` writes it).
//! async fn connect(State(ctx): State<Ctx>, Path(channel): Path<String>, upgrade: WebSocketUpgrade) -> Result<Response> {
//!     match channel.as_str() {
//!         "posts" => {}
//!         _ => return Err(Error::NotFound),
//!     }
//!     upgrade.connect(&ctx, &channel).await
//! }
//!
//! // Any handler or job: the new row goes to the top of every open index page.
//! async fn create(State(ctx): State<Ctx>) -> Result<&'static str> {
//!     let row_html = "<tr id=\"post_1\"><td>Hello</td></tr>";
//!     realtime::broadcast(&ctx, "posts", &realtime::prepend("posts", row_html)).await?;
//!     Ok("created")
//! }
//!
//! fn routes() -> Router<Ctx> {
//!     Router::new().route("/realtime/{channel}", get(connect)).route("/posts", post(create))
//! }
//! # let _ = routes;
//! ```
//!
//! In the page, htmx's WebSocket extension connects and swaps each message
//! into the element with the same `id` (`hx-swap-oob`), without custom
//! JavaScript. Load the extension in the layout's `<head>`, after htmx: a
//! page that loaded it itself would not connect when reached through an
//! `hx-boost` link.
//!
//! ```html
//! <!-- templates/layout.html, in <head> -->
//! <script src="https://unpkg.com/htmx-ext-ws@2.0.4/dist/ws.js" crossorigin="anonymous"></script>
//! <!-- the page -->
//! <div hx-ext="ws" ws-connect="/realtime/posts">
//!   <table><tbody id="posts">...<tr id="post_1">...</tr></tbody></table>
//! </div>
//! ```
//!
//! Messages: an element with an `id` replaces the page element with that id;
//! [`append`], [`prepend`],
//! [`update`] and [`remove`]
//! build the other swaps. One message may hold several of them.
//!
//! How it runs: one Durable Object of class [`OcreChannel`] (binding
//! `CHANNELS`) per channel name holds the channel's WebSockets with the
//! WebSocket Hibernation API, so it is evicted from memory, and costs no
//! duration, between broadcasts while browsers stay connected. It stores
//! nothing.
//!
//! Free plan (see [Realtime](https://ocre.rs/guides/realtime#free-plan-costs)): each connection (and
//! reconnection) and each broadcast is one Durable Object request (100,000 a
//! day); messages sent to browsers are free; a connection is also one Worker
//! request, while a broadcast is a subrequest of the request that sends it.
//! Needs Ocre's `realtime` feature and, in cloudflare.config.ts, the binding and
//! the SQLite-backed `OcreChannel` export (see [`OcreChannel`]).
//! API-only apps can use the same pieces and broadcast JSON.
//!
//! Rails' Action Cable pieces map as follows: the connect handler is the
//! connection (`identified_by` is [`WebSocketUpgrade::identified_by`], its
//! checks and `Result` are the callbacks and `rescue_from`), the channel
//! name and the route's query string are the channel params, client actions
//! (`perform`) are ordinary routes that broadcast, and
//! [`WebSocketUpgrade::rebroadcast`] relays what clients send to the other
//! subscribers. [`dev_routes`] lists recent broadcasts for tests.

use axum::{Router, extract::FromRequestParts, http::request::Parts};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

pub use crate::runtime::realtime::{OcreChannel, broadcast};

use crate::{
    Error,
    protect::is_websocket_upgrade,
    session::{Rejection, reject},
};

/// Name of the Durable Object binding holding the channels, declared in cloudflare.config.ts.
///
/// A missing binding makes [`broadcast`] and [`WebSocketUpgrade::connect`]
/// fail with [`Error::Internal`] naming the cloudflare.config.ts entries to add.
pub const CHANNELS_BINDING: &str = "CHANNELS";
/// Name of the Durable Object class Ocre exports for channels ([`OcreChannel`]).
///
/// The `exportName` of the `CHANNELS` binding and the `exports` key in
/// cloudflare.config.ts must use it.
pub const CHANNEL_CLASS: &str = "OcreChannel";
/// Prefix of every line Ocre logs about realtime, e.g. in `ocre dev` output.
///
/// Failed broadcasts log `[ocre realtime] broadcast to posts failed: ...`.
pub const LOG_PREFIX: &str = "[ocre realtime]";
/// Longest channel name, in bytes.
///
/// Channel names are 1 to 128 ASCII letters, digits, `_`, `-`, `.` or `:`
/// (e.g. `posts`, `post:12`), so they are safe in URLs and logs.
pub const MAX_CHANNEL_LEN: usize = 128;
/// Longest identity given to [`WebSocketUpgrade::identified_by`], in bytes.
pub const MAX_IDENTITY_LEN: usize = 256;
/// Longest client message [`WebSocketUpgrade::rebroadcast`] relays, in bytes; longer ones are dropped.
pub const MAX_REBROADCAST_BYTES: usize = 16 * 1024;

/// Header carrying a [`Subscriber`] from `connect` to the channel object.
pub(crate) const SUBSCRIBER_HEADER: &str = "X-Ocre-Subscriber";

/// What the channel object keeps with one WebSocket while it hibernates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Subscriber {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) identity: Option<String>,
    #[serde(default)]
    pub(crate) rebroadcast: bool,
}

impl Subscriber {
    /// The header value: the JSON, base64url-encoded so any identity is a valid header.
    pub(crate) fn to_header(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).expect("a subscriber serializes"))
    }

    /// The subscriber a header describes; a plain listener without one or when it is unreadable.
    pub(crate) fn from_header(value: Option<&str>) -> Self {
        value
            .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default()
    }

    /// What the other subscribers receive when this one sends `text`:
    /// `{"from": <identity or null>, "data": <text as JSON, or as a string>}`.
    /// `None` when this subscriber may not publish or `text` is too long.
    pub(crate) fn relay(&self, text: &str) -> Option<String> {
        if !self.rebroadcast || text.len() > MAX_REBROADCAST_BYTES {
            return None;
        }
        let data = serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.to_owned()));
        Some(serde_json::json!({ "from": self.identity, "data": data }).to_string())
    }

    /// What the other subscribers receive when this one connects (`"joined"`)
    /// or disconnects (`"left"`): `{"event": .., "from": <identity>}`, for
    /// identified publishers only.
    pub(crate) fn presence(&self, event: &str) -> Option<String> {
        let identity = self.identity.as_ref().filter(|_| self.rebroadcast)?;
        Some(serde_json::json!({ "event": event, "from": identity }).to_string())
    }
}

/// Why `name` is not a valid channel name, or `None`: 1 to 128 ASCII letters,
/// digits and `_ - . :`, so names are safe in URLs and logs.
pub(crate) fn channel_error(name: &str) -> Option<String> {
    let valid_chars = name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'));
    if name.is_empty() || name.len() > MAX_CHANNEL_LEN || !valid_chars {
        Some(format!(
            "invalid channel name `{name}`: use 1 to {MAX_CHANNEL_LEN} ASCII letters, digits, `_`, `-`, `.` or `:`"
        ))
    } else {
        None
    }
}

/// Error for a missing `CHANNELS` Durable Object binding.
pub(crate) fn missing_binding(detail: &str) -> Error {
    Error::internal(format!(
        "Durable Object binding `{CHANNELS_BINDING}` is missing ({detail}). Fix: add to cloudflare.config.ts \
         `{CHANNELS_BINDING}: bindings.durableObject({{ worker: \"<app>\", exportName: \"{CHANNEL_CLASS}\" }}),` in worker.env and \
         `{CHANNEL_CLASS}: exports.durableObject({{ storage: \"sqlite\" }}),` in worker.exports \
         (`ocre g scaffold <Model> ... --realtime` adds them)"
    ))
}

/// A close code the server may send back: the client's own code, or 1000
/// (normal closure) for codes that must not appear in a Close frame.
pub(crate) fn close_code(code: usize) -> u16 {
    match code {
        1000..=1003 | 1007..=1014 | 3000..=4999 => code as u16,
        _ => 1000,
    }
}

/// Axum extractor for a WebSocket handshake (`Upgrade: websocket`), finished with [`connect`](Self::connect).
///
/// Check who may listen in the handler, then call
/// [`connect`](Self::connect). Browsers send the session cookie with the
/// handshake, so `CurrentUser` and [`Session`](crate::Session) work in the
/// same handler. [`serve`](crate::serve) refuses handshakes from other sites
/// (403), as it does for forms.
///
/// Rejection: requests without `Upgrade: websocket` get
/// [`Error::BadRequest`] (400), rendered as an HTML page in full-stack apps
/// and as JSON in API-only apps.
///
/// Free plan: each connection (and each reconnection) is one Worker request
/// and one Durable Object request (100,000 a day each); hibernated sockets
/// cost nothing between messages.
///
/// # Examples
///
/// ```no_run
/// use axum::{extract::{Path, State}, response::Response};
/// use ocre::{Ctx, Result, realtime::WebSocketUpgrade};
///
/// // GET /realtime/{channel}
/// async fn connect(State(ctx): State<Ctx>, Path(channel): Path<String>, upgrade: WebSocketUpgrade)
///     -> Result<Response> {
///     upgrade.connect(&ctx, &channel).await
/// }
/// ```
#[derive(Debug)]
pub struct WebSocketUpgrade {
    pub(crate) subscriber: Subscriber,
}

impl WebSocketUpgrade {
    /// Names who is connecting, like Action Cable's `identified_by :current_user`.
    ///
    /// The identity (a user id, a display name...) stays with the socket in
    /// the channel object, even while it hibernates, and is the `from` of
    /// the messages [`rebroadcast`](Self::rebroadcast) relays, so clients
    /// cannot forge it. At most [`MAX_IDENTITY_LEN`] bytes; a longer one
    /// makes [`connect`](Self::connect) fail with [`Error::Internal`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::{extract::{Path, State}, response::Response};
    /// use ocre::{Ctx, Result, realtime::WebSocketUpgrade};
    /// # struct CurrentUser { id: i64 }
    ///
    /// async fn connect(State(ctx): State<Ctx>, Path(channel): Path<String>, upgrade: WebSocketUpgrade)
    ///     -> Result<Response> {
    ///     let user = CurrentUser { id: 7 }; // `user: CurrentUser` as an extractor after `ocre g auth`
    ///     upgrade.identified_by(user.id.to_string()).connect(&ctx, &channel).await
    /// }
    /// ```
    pub fn identified_by(mut self, identity: impl Into<String>) -> Self {
        self.subscriber.identity = Some(identity.into());
        self
    }

    /// Relays what this client sends to the channel's other subscribers, like a channel that rebroadcasts client data.
    ///
    /// Without it, messages from clients are ignored (they only listen). With
    /// it, each text message of at most [`MAX_REBROADCAST_BYTES`] is sent to
    /// every other socket of the channel as JSON:
    /// `{"from": "<identity>", "data": <message>}`, where `from` is the
    /// [`identified_by`](Self::identified_by) identity (`null` without one)
    /// and `data` is the message parsed as JSON (a string when it is not
    /// JSON). An identified publisher's arrival and departure reach the
    /// others as `{"event": "joined", "from": "<identity>"}` and
    /// `{"event": "left", ...}` (presence, Action Cable's `subscribed` and
    /// `unsubscribed` hooks). No app code runs: use it for typing indicators, cursors or chat
    /// between JavaScript clients; send anything that must be checked or
    /// stored to an ordinary route that saves it and calls [`broadcast`].
    /// Relayed messages are never HTML swaps, so a client cannot inject
    /// markup into other pages.
    ///
    /// Free plan: Cloudflare bills incoming WebSocket messages to a Durable
    /// Object at 20 messages per request (100,000 requests a day); relaying
    /// is free.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::{extract::{Path, State}, response::Response};
    /// use ocre::{Ctx, Result, realtime::WebSocketUpgrade};
    ///
    /// // GET /chat/{room}: `ws.send(JSON.stringify({text: "hi"}))` reaches the others as
    /// // {"from":"ada","data":{"text":"hi"}}.
    /// async fn chat(State(ctx): State<Ctx>, Path(room): Path<String>, upgrade: WebSocketUpgrade)
    ///     -> Result<Response> {
    ///     upgrade.identified_by("ada").rebroadcast().connect(&ctx, &format!("chat:{room}")).await
    /// }
    /// ```
    pub fn rebroadcast(mut self) -> Self {
        self.subscriber.rebroadcast = true;
        self
    }

    /// Checks the identity before connecting.
    pub(crate) fn subscriber_header(&self) -> Result<String, Error> {
        match &self.subscriber.identity {
            Some(identity) if identity.len() > MAX_IDENTITY_LEN => Err(Error::internal(format!(
                "realtime identity is {} bytes, more than {MAX_IDENTITY_LEN}. Fix: identify subscribers by a short \
                 value such as the user id",
                identity.len()
            ))),
            _ => Ok(self.subscriber.to_header()),
        }
    }
}

impl<S: Sync> FromRequestParts<S> for WebSocketUpgrade {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Rejection> {
        if is_websocket_upgrade(&parts.headers) {
            Ok(Self { subscriber: Subscriber::default() })
        } else {
            Err(reject(Error::bad_request(
                "Expected a WebSocket connection (`Upgrade: websocket`). Connect with htmx's ws extension or `new WebSocket(url)`.",
            )))
        }
    }
}

/// Development endpoint listing recent broadcasts, served by `ocre dev` only, for tests (Rails' `assert_broadcasts`).
///
/// `GET /ocre/dev/realtime/sent.json` answers the last 50 messages
/// [`broadcast`] sent from this Worker instance (requests and jobs alike),
/// oldest first: `[{"id": 1, "channel": "posts", "message": "<tr ...>"}]`.
/// An end-to-end test triggers a change, then checks what went out without
/// opening a WebSocket (`ocre::testing::Client` reads it). Only successful
/// broadcasts are listed.
///
/// Debug builds only (`ocre dev`); release builds (`ocre deploy`) get an
/// empty router, so it is a 404 in production. `ocre g scaffold ...
/// --realtime` merges it into the routes of `src/realtime.rs`. It uses no
/// billed resource: the list lives in the Worker's memory.
///
/// # Examples
///
/// ```
/// use axum::{Router, routing::get};
/// use ocre::Ctx;
///
/// # async fn connect() {}
/// fn routes() -> Router<Ctx> {
///     Router::new().route("/realtime/{channel}", get(connect)).merge(ocre::realtime::dev_routes())
/// }
/// # let _ = routes;
/// ```
pub fn dev_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    #[cfg(not(debug_assertions))]
    {
        Router::new()
    }
    #[cfg(debug_assertions)]
    Router::new().route("/ocre/dev/realtime/sent.json", axum::routing::get(|| async { axum::Json(dev::sent()) }))
}

/// Remembers a sent broadcast for [`dev_routes`] (debug builds only).
pub(crate) fn record(channel: &str, message: &str) {
    #[cfg(debug_assertions)]
    dev::record(channel, message);
    #[cfg(not(debug_assertions))]
    let _ = (channel, message);
}

#[cfg(debug_assertions)]
mod dev {
    use std::sync::{Mutex, PoisonError};

    use serde::Serialize;

    /// How many broadcasts are kept.
    const KEEP: usize = 50;

    #[derive(Debug, Clone, Serialize)]
    pub(super) struct Sent {
        id: u64,
        channel: String,
        message: String,
    }

    static SENT: Mutex<(u64, Vec<Sent>)> = Mutex::new((1, Vec::new()));

    pub(super) fn record(channel: &str, message: &str) {
        let mut sent = SENT.lock().unwrap_or_else(PoisonError::into_inner);
        let id = sent.0;
        sent.0 += 1;
        sent.1.push(Sent { id, channel: channel.to_owned(), message: message.to_owned() });
        if sent.1.len() > KEEP {
            sent.1.remove(0);
        }
    }

    pub(super) fn sent() -> Vec<Sent> {
        SENT.lock().unwrap_or_else(PoisonError::into_inner).1.clone()
    }
}

/// Builds a message that inserts `html` at the end of the element with id `target` (htmx `beforeend`).
///
/// The fragment is wrapped in an element the browser can parse it in (a
/// `<tr>` goes in a `<tbody>`, an `<li>` in a `<ul>`, and likewise for other
/// table parts, options and `<dt>`/`<dd>`; anything else in a `<div>`)
/// carrying `hx-swap-oob`; htmx drops the wrapper. `target` is
/// escaped; `html` is sent as is, so escape user content in it (askama
/// templates do). Pure: no I/O until you [`broadcast`] it.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     ocre::realtime::append("comments", "<li id=\"comment_2\">Hi</li>"),
///     "<ul hx-swap-oob=\"beforeend:#comments\"><li id=\"comment_2\">Hi</li></ul>"
/// );
/// ```
pub fn append(target: &str, html: &str) -> String {
    swap("beforeend", target, html)
}

/// Builds a message that inserts `html` at the start of the element with id `target` (htmx `afterbegin`).
///
/// Typical use: a new row at the top of a table body. Wrapping and escaping
/// work as in [`append`].
///
/// # Examples
///
/// ```
/// assert_eq!(
///     ocre::realtime::prepend("posts", "<tr id=\"post_3\"><td>Hi</td></tr>"),
///     "<tbody hx-swap-oob=\"afterbegin:#posts\"><tr id=\"post_3\"><td>Hi</td></tr></tbody>"
/// );
/// ```
pub fn prepend(target: &str, html: &str) -> String {
    swap("afterbegin", target, html)
}

/// Builds a message that replaces the contents of the element with id `target` (htmx `innerHTML`).
///
/// The element itself stays; wrapping and escaping work as in [`append`].
/// To replace a whole element, broadcast its new HTML with the same `id`
/// instead.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     ocre::realtime::update("post_count", "3 posts"),
///     "<div hx-swap-oob=\"innerHTML:#post_count\">3 posts</div>"
/// );
/// ```
pub fn update(target: &str, html: &str) -> String {
    swap("innerHTML", target, html)
}

/// Builds a message that removes the element with id `id` from the page (htmx `delete`).
///
/// `id` is escaped.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::realtime::remove("post_3"), "<div id=\"post_3\" hx-swap-oob=\"delete\"></div>");
/// ```
pub fn remove(id: &str) -> String {
    format!("<div id=\"{}\" hx-swap-oob=\"delete\"></div>", escape(id))
}

/// Wraps `html` in an element the browser can parse it in (a `<tr>` only
/// parses inside a table body), carrying the htmx swap. htmx drops the
/// wrapper and swaps its children.
fn swap(strategy: &str, target: &str, html: &str) -> String {
    let wrapper = wrapper_for(html);
    format!("<{wrapper} hx-swap-oob=\"{strategy}:#{}\">{html}</{wrapper}>", escape(target))
}

/// The parent element `html`'s first tag needs.
fn wrapper_for(html: &str) -> &'static str {
    let tag: String = html
        .trim_start()
        .strip_prefix('<')
        .unwrap_or_default()
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match tag.as_str() {
        "tr" => "tbody",
        "td" | "th" => "tr",
        "tbody" | "thead" | "tfoot" | "caption" | "colgroup" => "table",
        "li" => "ul",
        "option" | "optgroup" => "select",
        "dt" | "dd" => "dl",
        _ => "div",
    }
}

/// Escapes an attribute value.
fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

#[cfg(test)]
#[path = "../tests/realtime.rs"]
mod tests;
