use axum::response::Response;
use wasm_bindgen::JsValue;
use worker::{Env, Headers, Method, Request, RequestInit, Stub, send::SendFuture};

use super::Ctx;
use crate::{
    Error, Result,
    realtime::{CHANNELS_BINDING, LOG_PREFIX, WebSocketUpgrade, channel_error, missing_binding},
};

// `#[durable_object]` generates public wasm-bindgen glue (constructor and runtime callbacks) inside
// a `const _` block, which cannot carry docs and which an `allow` on the struct does not reach.
#[allow(missing_docs)]
mod channel;

pub use channel::OcreChannel;

/// Address of requests from the Worker to a channel object; only the method matters.
const CHANNEL_URL: &str = "https://ocre-channel/";

/// The channel object for `channel`.
fn stub(env: &Env, channel: &str) -> Result<Stub> {
    let namespace = env.durable_object(CHANNELS_BINDING).map_err(|err| missing_binding(&err.to_string()))?;
    Ok(namespace.get_by_name(channel)?)
}

/// Sends `message` (an HTML fragment or JSON text) to every browser connected to `channel`.
///
/// Returns once the channel's [`OcreChannel`] object has sent it. Build HTML
/// messages with [`prepend`](crate::realtime::prepend),
/// [`append`](crate::realtime::append), [`update`](crate::realtime::update),
/// [`remove`](crate::realtime::remove), or an element with an `id` that
/// replaces the page element with that id; several can go in one message.
/// Works from handlers and jobs. The returned future is `Send`, so axum
/// handlers can await it.
///
/// Every failure is logged as `[ocre realtime] broadcast to <channel> failed:
/// ...` and returned; callers that treat updates as best effort (the
/// generated controllers do) can ignore it with `.ok()`.
///
/// Free plan: each call is one Durable Object request (100,000 a day), even
/// when the channel has no subscribers, and one subrequest of the current
/// request; the messages to browsers are free.
///
/// # Errors
///
/// - [`Error::Internal`] when `channel` is not a valid name (1 to
///   [`MAX_CHANNEL_LEN`](crate::realtime::MAX_CHANNEL_LEN) ASCII letters,
///   digits, `_`, `-`, `.` or `:`).
/// - [`Error::Internal`] when the `CHANNELS` Durable Object binding is
///   missing; the message names the wrangler.toml entries to add.
/// - [`Error::Internal`] when the channel object cannot be reached (e.g. the
///   free-plan quota is exhausted) or answers with a non-200 status.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::{Path, State};
/// use ocre::{Ctx, Result, realtime};
///
/// async fn destroy(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<()> {
///     // ... delete the post, then remove its row from every open index page:
///     realtime::broadcast(&ctx, "posts", &realtime::remove(&format!("post_{id}"))).await.ok();
///     Ok(())
/// }
/// ```
pub fn broadcast(ctx: &Ctx, channel: &str, message: &str) -> impl Future<Output = Result<()>> + Send + use<> {
    let env = ctx.env().clone();
    let invalid = channel_error(channel);
    let channel = channel.to_owned();
    let message = message.to_owned();
    SendFuture::new(async move {
        let result = match invalid {
            Some(invalid) => Err(Error::internal(invalid)),
            None => send(&env, &channel, &message).await,
        };
        if let Err(err) = &result {
            worker::console_error!("{LOG_PREFIX} broadcast to {channel} failed: {err}");
        }
        result
    })
}

async fn send(env: &Env, channel: &str, message: &str) -> Result<()> {
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(JsValue::from_str(message)));
    let mut response = stub(env, channel)?.fetch_with_request(Request::new_with_init(CHANNEL_URL, &init)?).await?;
    match response.status_code() {
        200 => Ok(()),
        status => Err(Error::internal(format!("channel object answered {status}: {}", response.text().await?))),
    }
}

impl WebSocketUpgrade {
    /// Connects the browser to `channel`, returning the `101 Switching Protocols` response to send back.
    ///
    /// The handshake is forwarded to the channel's [`OcreChannel`] object,
    /// which accepts the WebSocket (hibernating). Check who may listen before
    /// calling it: list the allowed channels, or use a channel per record or
    /// user (`post:12`). The returned future is `Send`.
    ///
    /// Free plan: one Durable Object request per connection and reconnection.
    ///
    /// # Errors
    ///
    /// - [`Error::BadRequest`] (400) when `channel` is not a valid name (1 to
    ///   [`MAX_CHANNEL_LEN`](crate::realtime::MAX_CHANNEL_LEN) ASCII letters,
    ///   digits, `_`, `-`, `.` or `:`).
    /// - [`Error::Internal`] (500) when the `CHANNELS` Durable Object binding
    ///   is missing (the message names the wrangler.toml entries to add) or
    ///   the channel object cannot be reached.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::{extract::{Path, State}, response::Response};
    /// use ocre::{Ctx, Error, Result, realtime::WebSocketUpgrade};
    ///
    /// async fn connect(State(ctx): State<Ctx>, Path(channel): Path<String>, upgrade: WebSocketUpgrade)
    ///     -> Result<Response> {
    ///     match channel.as_str() {
    ///         "posts" => {}
    ///         _ => return Err(Error::NotFound),
    ///     }
    ///     upgrade.connect(&ctx, &channel).await
    /// }
    /// ```
    pub fn connect(self, ctx: &Ctx, channel: &str) -> impl Future<Output = Result<Response>> + Send + use<> {
        let env = ctx.env().clone();
        let invalid = channel_error(channel);
        let channel = channel.to_owned();
        SendFuture::new(async move {
            if let Some(invalid) = invalid {
                return Err(Error::bad_request(invalid));
            }
            let headers = Headers::new();
            headers.set("Upgrade", "websocket")?;
            let mut init = RequestInit::new();
            init.with_headers(headers);
            let response =
                stub(&env, &channel)?.fetch_with_request(Request::new_with_init(CHANNEL_URL, &init)?).await?;
            Ok(Response::from(response))
        })
    }
}
