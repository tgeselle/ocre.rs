//! The `OcreChannel` Durable Object class. It sits in its own module because
//! `#[durable_object]` generates public wasm-bindgen glue that cannot carry
//! docs; the parent allows `missing_docs` for this module only.

use worker::{
    DurableObject, Env, Method, Request, State, WebSocket, WebSocketIncomingMessage, WebSocketPair, durable_object,
};

use crate::realtime::{SUBSCRIBER_HEADER, Subscriber, close_code};

/// The Durable Object class behind each realtime channel, exported by Ocre under the name `OcreChannel`.
///
/// One instance per channel name holds that channel's WebSockets. It
/// accepts them with the WebSocket Hibernation API, so it is evicted from
/// memory between broadcasts while browsers stay connected (hibernated
/// sockets cost no duration), and it stores nothing but each socket's
/// attachment (its identity and whether it may publish). What subscribers
/// send is ignored, or relayed to the others as JSON for sockets connected
/// with [`WebSocketUpgrade::rebroadcast`](crate::realtime::WebSocketUpgrade::rebroadcast);
/// it completes the closing handshakes browsers start. Apps never
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
        let subscriber = Subscriber::from_header(req.headers().get(SUBSCRIBER_HEADER)?.as_deref());
        let pair = WebSocketPair::new()?;
        if let Some(joined) = subscriber.presence("joined") {
            self.send_to_others(&pair.server, &joined);
        }
        self.state.accept_web_socket(&pair.server);
        pair.server.serialize_attachment(&subscriber)?;
        worker::Response::from_websocket(pair.client)
    }

    /// Relays text from sockets allowed to publish to every other socket; ignores the rest.
    async fn websocket_message(&self, ws: WebSocket, message: WebSocketIncomingMessage) -> worker::Result<()> {
        let WebSocketIncomingMessage::String(text) = message else {
            return Ok(());
        };
        let subscriber: Subscriber = ws.deserialize_attachment()?.unwrap_or_default();
        let Some(relayed) = subscriber.relay(&text) else {
            return Ok(());
        };
        self.send_to_others(&ws, &relayed);
        Ok(())
    }

    /// Completes the closing handshake the browser started.
    async fn websocket_close(&self, ws: WebSocket, code: usize, reason: String, _clean: bool) -> worker::Result<()> {
        self.announce_departure(&ws);
        // Already closed when the runtime auto-replied (compatibility date 2026-04-07 or later).
        let _ = ws.close(Some(close_code(code)), Some(reason));
        Ok(())
    }

    async fn websocket_error(&self, ws: WebSocket, _error: worker::Error) -> worker::Result<()> {
        self.announce_departure(&ws);
        Ok(())
    }
}

impl OcreChannel {
    /// Sends `text` to every socket of the channel but `sender`.
    fn send_to_others(&self, sender: &WebSocket, text: &str) {
        let sender: &worker::web_sys::WebSocket = sender.as_ref();
        for other in self.state.get_websockets() {
            let target: &worker::web_sys::WebSocket = other.as_ref();
            if target != sender {
                let _ = other.send_with_str(text);
            }
        }
    }

    /// Tells the others that an identified publisher left.
    fn announce_departure(&self, ws: &WebSocket) {
        let subscriber: Subscriber = ws.deserialize_attachment().ok().flatten().unwrap_or_default();
        if let Some(left) = subscriber.presence("left") {
            self.send_to_others(ws, &left);
        }
    }
}
