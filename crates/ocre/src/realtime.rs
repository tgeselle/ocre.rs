//! Realtime updates over WebSockets, like Rails' Action Cable and Turbo
//! Streams: browsers subscribe to a named channel, and handlers or jobs
//! [`broadcast`] HTML fragments (or JSON) to every subscriber.
//!
//! ```ignore
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
//! realtime::broadcast(&ctx, "posts", &realtime::prepend("posts", &row_html)).await?;
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
//! [`append`], [`prepend`], [`update`] and [`remove`] build the other swaps.
//! One message may hold several of them.
//!
//! How it runs: one Durable Object of class [`OcreChannel`] (binding
//! `CHANNELS`) per channel name holds the channel's WebSockets with the
//! WebSocket Hibernation API, so it sleeps, and costs no duration, between
//! broadcasts. Each connection and each broadcast is one Durable Object
//! request (free plan: 100,000 a day); messages sent to browsers are free.
//! Needs Ocre's `realtime` feature and, in wrangler.toml, the binding and a
//! `new_sqlite_classes` migration for `OcreChannel`.

use axum::{extract::FromRequestParts, http::request::Parts};

pub use crate::runtime::realtime::{OcreChannel, broadcast};

use crate::{
    Error,
    protect::is_websocket_upgrade,
    session::{Rejection, reject},
};

/// Name of the Durable Object binding holding the channels.
pub const CHANNELS_BINDING: &str = "CHANNELS";
/// Durable Object class Ocre exports for channels.
pub const CHANNEL_CLASS: &str = "OcreChannel";
/// Prefix of every line Ocre logs about realtime, e.g. in `ocre dev` output.
pub const LOG_PREFIX: &str = "[ocre realtime]";
/// Longest channel name.
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
        "Durable Object binding `{CHANNELS_BINDING}` is missing ({detail}). Fix: add to wrangler.toml \
         [[durable_objects.bindings]] name = \"{CHANNELS_BINDING}\", class_name = \"{CHANNEL_CLASS}\" and \
         [[migrations]] tag = \"ocre-realtime-v1\", new_sqlite_classes = [\"{CHANNEL_CLASS}\"] \
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

/// Extractor for a WebSocket handshake (`Upgrade: websocket`). Other
/// requests get a 400. Finish the handshake with
/// [`connect`](Self::connect), after checking who may listen:
///
/// ```ignore
/// async fn connect(State(ctx): State<Ctx>, Path(channel): Path<String>, upgrade: WebSocketUpgrade)
///     -> Result<Response> {
///     upgrade.connect(&ctx, &channel).await
/// }
/// ```
///
/// Browsers send the session cookie with the handshake, so `CurrentUser`
/// and `Session` work in the same handler. `ocre::serve` refuses handshakes
/// from other sites (403), as it does for forms.
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

/// Inserts `html` at the end of the element with id `target`
/// (htmx `beforeend`).
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

/// Inserts `html` at the start of the element with id `target`
/// (htmx `afterbegin`), e.g. a new row at the top of a table body.
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

/// Replaces the contents of the element with id `target` (htmx `innerHTML`).
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

/// Removes the element with id `id` from the page.
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
