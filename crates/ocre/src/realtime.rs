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
//! into the element with the same `id` (`hx-swap-oob`), without custom JavaScript:
//!
//! ```html
//! <script src="https://unpkg.com/htmx-ext-ws@2.0.4/dist/ws.js" crossorigin="anonymous"></script>
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
//! Free plan (see the README's Realtime section): each connection (and
//! reconnection) and each broadcast is one Durable Object request (100,000 a
//! day); messages sent to browsers are free; a connection is also one Worker
//! request, while a broadcast is a subrequest of the request that sends it.
//! Needs Ocre's `realtime` feature and, in cloudflare.config.ts, the binding and
//! the SQLite-backed `OcreChannel` export (see [`OcreChannel`]).
//! API-only apps can use the same pieces and broadcast JSON.

use axum::{extract::FromRequestParts, http::request::Parts};

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
    _private: (),
}

impl<S: Sync> FromRequestParts<S> for WebSocketUpgrade {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Rejection> {
        if is_websocket_upgrade(&parts.headers) {
            Ok(Self { _private: () })
        } else {
            Err(reject(Error::bad_request(
                "Expected a WebSocket connection (`Upgrade: websocket`). Connect with htmx's ws extension or `new WebSocket(url)`.",
            )))
        }
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
