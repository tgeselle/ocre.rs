//! The `OcreChannel` Durable Object class. It sits in its own module because
//! `#[durable_object]` generates public wasm-bindgen glue that cannot carry
//! docs; the parent allows `missing_docs` for this module only.

use worker::{
    DurableObject, Env, Method, Request, State, WebSocket, WebSocketIncomingMessage, WebSocketPair, durable_object,
};

use crate::realtime::close_code;

/// The Durable Object class behind each realtime channel, exported by Ocre under the name `OcreChannel`.
///
/// One instance per channel name holds that channel's WebSockets. It
/// accepts them with the WebSocket Hibernation API, so it is evicted from
/// memory between broadcasts while browsers stay connected (hibernated
/// sockets cost no duration), and it stores nothing. What subscribers send
/// is ignored; it completes the closing handshakes browsers start. Apps never
/// call it directly: they use [`broadcast`](crate::realtime::broadcast) and
/// [`WebSocketUpgrade::connect`](crate::realtime::WebSocketUpgrade::connect).
///
/// Free plan: Durable Objects must be SQLite-backed (`storage: "sqlite"`);
/// one object accepts up to 32,768 WebSockets. cloudflare.config.ts declares it
/// (`ocre g scaffold ... --realtime` adds this, with `demo` as the app name):
///
/// ```ts
/// // worker.env
/// CHANNELS: bindings.durableObject({ worker: "demo", exportName: "OcreChannel" }),
/// // worker.exports
/// OcreChannel: exports.durableObject({ storage: "sqlite" }),
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
