use axum::response::Response;
use wasm_bindgen::JsValue;
use worker::{
    DurableObject, Env, Headers, Method, Request, RequestInit, State, Stub, WebSocket, WebSocketIncomingMessage,
    WebSocketPair, durable_object, send::SendFuture,
};

use super::Ctx;
use crate::{
    Error, Result,
    realtime::{CHANNELS_BINDING, LOG_PREFIX, WebSocketUpgrade, channel_error, close_code, missing_binding},
};

/// Address of requests from the Worker to a channel object; only the method matters.
const CHANNEL_URL: &str = "https://ocre-channel/";

/// The Durable Object behind each realtime channel, exported by Ocre (feature
/// `realtime`): one instance per channel name holds that channel's
/// WebSockets. It accepts them with the WebSocket Hibernation API, so it is
/// evicted from memory between broadcasts while browsers stay connected, and
/// it stores nothing. Apps never call it directly: they use
/// [`broadcast`] and [`WebSocketUpgrade::connect`]. wrangler.toml declares it:
///
/// ```toml
/// [[durable_objects.bindings]]
/// name = "CHANNELS"
/// class_name = "OcreChannel"
///
/// [[migrations]]
/// tag = "ocre-realtime-v1"
/// new_sqlite_classes = ["OcreChannel"]
/// ```
#[durable_object(websocket)]
pub struct OcreChannel {
    state: State,
}

impl DurableObject for OcreChannel {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }

    /// `POST`: send the body to every socket, answer how many got it.
    /// Anything else: a WebSocket handshake forwarded by `connect`.
    async fn fetch(&self, mut req: Request) -> worker::Result<worker::Response> {
        if req.method() == Method::Post {
            let message = req.text().await?;
            let sent = self.state.get_websockets().iter().filter(|ws| ws.send_with_str(&message).is_ok()).count();
            return worker::Response::ok(sent.to_string());
        }
        let pair = WebSocketPair::new()?;
        self.state.accept_web_socket(&pair.server);
        worker::Response::from_websocket(pair.client)
    }

    /// Subscribers only listen; what they send is ignored.
    async fn websocket_message(&self, _ws: WebSocket, _message: WebSocketIncomingMessage) -> worker::Result<()> {
        Ok(())
    }

    /// Completes the closing handshake the browser started.
    async fn websocket_close(&self, ws: WebSocket, code: usize, reason: String, _clean: bool) -> worker::Result<()> {
        // Already closed when the runtime auto-replied (compatibility date 2026-04-07 or later).
        let _ = ws.close(Some(close_code(code)), Some(reason));
        Ok(())
    }

    async fn websocket_error(&self, _ws: WebSocket, _error: worker::Error) -> worker::Result<()> {
        Ok(())
    }
}

/// The channel object for `channel`.
fn stub(env: &Env, channel: &str) -> Result<Stub> {
    let namespace = env.durable_object(CHANNELS_BINDING).map_err(|err| missing_binding(&err.to_string()))?;
    Ok(namespace.get_by_name(channel)?)
}

/// Sends `message` (an HTML fragment or JSON text) to every browser connected
/// to `channel`, and returns when the channel object has sent it. Channels
/// without subscribers cost one Durable Object request and send nothing.
///
/// ```ignore
/// let row = render(&RowView { post: &post })?.0;
/// ocre::realtime::broadcast(&ctx, "posts", &ocre::realtime::prepend("posts", &row)).await?;
/// ```
///
/// Failures (invalid channel name, missing binding, exhausted free-plan
/// quota) are logged with an `[ocre realtime]` line and returned; callers
/// that treat updates as best effort can ignore them with `.ok()`.
/// The returned future is `Send`, so axum handlers can await it.
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
    /// Connects the browser to `channel`: the channel object accepts the
    /// WebSocket and the handler returns its `101 Switching Protocols`
    /// response. An invalid channel name is a 400. Check who may listen
    /// before calling it.
    ///
    /// ```ignore
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
