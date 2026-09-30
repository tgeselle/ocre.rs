# Realtime

Ocre pushes HTML to open pages over WebSockets, like Rails' Action Cable and Turbo Streams: handlers and jobs broadcast fragments to a named channel, and htmx swaps them into every page subscribed to it. This page covers the `--realtime` scaffold, how channels run on a hibernating Durable Object, the broadcast helpers, authorization of channels, and what it costs on the free plan.

## Before you start

- An Ocre app created with `ocre new`, full-stack (the default). API-only apps can use the same pieces by hand; see [Without the scaffold](#without-the-scaffold-and-in-api-only-apps).
- Nothing to set up on Cloudflare: the first `--realtime` scaffold adds everything to `Cargo.toml` and `cloudflare.config.ts`, and `ocre deploy` creates the Durable Object namespace.
- The private-channel example uses `crate::auth::OptionalUser`, created by `ocre g auth` (see [Authentication](authentication.md)); the job example needs a job from `ocre g job` (see [Background jobs and schedules](jobs.md)).

## How it works

```text
browser ──WebSocket──> GET /realtime/messages ──> src/realtime.rs connect (who may listen)
                                                     └─> OcreChannel "messages" (Durable Object, holds the sockets)
POST /messages ──> create ──> ocre::realtime::broadcast(&ctx, "messages", html) ──> every socket
```

- **One Durable Object per channel.** Ocre ships a Durable Object class, `OcreChannel`, bound as `CHANNELS`. Each channel name (`messages`, `post:12`) gets its own instance, which accepts the channel's WebSockets.
- **Hibernation.** The object accepts sockets with the [WebSocket Hibernation API](https://developers.cloudflare.com/durable-objects/best-practices/websockets/): between broadcasts it is evicted from memory while browsers stay connected, and hibernated sockets cost no duration. It stores nothing.
- **Broadcast.** `ocre::realtime::broadcast(&ctx, channel, message)` sends one request to the channel's object, which sends the text to every socket. It returns once the object has sent it.
- **Clients listen.** What a browser sends over the socket is ignored, unless its `connect` enabled relaying (see [Identifying subscribers and relaying client messages](#identifying-subscribers-and-relaying-client-messages)); the object completes the closing handshakes browsers start. Use forms and htmx requests to send data.
- **Client side**: htmx's [WebSocket extension](https://htmx.org/extensions/ws/) connects, reconnects with backoff, and swaps each message into the element with the same `id` ([`hx-swap-oob`](https://htmx.org/attributes/hx-swap-oob/)). No custom JavaScript.

## Generating a live resource

```sh
ocre g scaffold Message body:text --realtime
```

```text
  create  migrations/0004_create_messages.sql
  create  src/models/message.rs
  create  src/messages.rs
  create  templates/messages/index.html
  create  templates/messages/show.html
  create  templates/messages/new.html
  create  templates/messages/edit.html
  create  templates/messages/_form.html
  create  templates/messages/_row.html
  create  src/realtime.rs
  update  src/models/mod.rs
  update  src/lib.rs
  update  Cargo.toml
  update  cloudflare.config.ts

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/messages
  open http://localhost:8787/messages in a second window, then create a message
```

`/messages` now shows the messages other visitors create, edit and delete without reloading. Compared with a plain scaffold, `--realtime` adds:

| Piece | What it does |
|---|---|
| `templates/messages/_row.html` | One table row with `id="message_<id>"`, included by the index and rendered for broadcasts |
| `templates/messages/index.html` | Loads the htmx ws extension and wraps the table in `<div hx-ext="ws" ws-connect="/realtime/messages">`; the `<tbody>` has `id="messages"` |
| `src/messages.rs` | After a successful create, broadcasts `prepend("messages", row)`; after update, the new row (same `id`, so it replaces the old one); after delete, `remove("message_<id>")` |
| `src/realtime.rs` (first use) | `GET /realtime/{channel}` routed to `connect`, which lists the channels anyone may open; later `--realtime` scaffolds add their channel under `// ocre:channels` |
| `Cargo.toml` (first use) | Ocre's `realtime` feature |
| `cloudflare.config.ts` (first use) | The `CHANNELS` Durable Object binding and the `OcreChannel` export |

The `cloudflare.config.ts` entries (for an app named `chat`):

```ts
// in worker.env, after // ocre:env
// The realtime channels' Durable Objects (`ocre::realtime`).
CHANNELS: bindings.durableObject({ worker: "chat", exportName: "OcreChannel" }),

// in worker.exports, after // ocre:exports
// Realtime channels (`ocre::realtime`): one Durable Object per channel holds the
// browsers' WebSockets, hibernated between broadcasts so idle connections cost
// no duration. The free plan only accepts SQLite-backed classes.
OcreChannel: exports.durableObject({ storage: "sqlite" }),
```

The generated index page (`templates/messages/index.html`):

```html
{# Live updates: htmx's WebSocket extension swaps in the rows other visitors create, edit and delete (src/realtime.rs). #}
<script src="https://unpkg.com/htmx-ext-ws@2.0.4/dist/ws.js" crossorigin="anonymous"></script>
<div hx-ext="ws" ws-connect="/realtime/messages">
<table>
  <thead>
    <tr><th>Body</th><th></th></tr>
  </thead>
  <tbody id="messages">
    {% for message in messages %}
    {% include "messages/_row.html" %}
    {% endfor %}
  </tbody>
</table>
</div>
```

The broadcasting part of the generated controller (`src/messages.rs`, shortened):

```rust
/// One table row; the index page and live updates share it.
#[derive(Template)]
#[template(path = "messages/_row.html")]
struct RowView<'a> {
    message: &'a Message,
}

fn row(record: &Message) -> Result<String> {
    Ok(render(&RowView { message: record })?.0)
}

/// Sends `html` to every open index page (channel `messages`, see
/// src/realtime.rs). Best effort: a failed broadcast is logged by Ocre and
/// never fails the request.
async fn broadcast(ctx: &Ctx, html: &str) {
    realtime::broadcast(ctx, "messages", html).await.ok();
}

// in create, after message::create succeeded:
broadcast(&ctx, &realtime::prepend("messages", &row(&record)?)).await;
// in update:
broadcast(&ctx, &row(&record)?).await;
// in delete:
broadcast(&ctx, &realtime::remove(&format!("message_{id}"))).await;
```

`ocre dev` runs the Durable Object locally (workerd supports Durable Objects and WebSocket Hibernation); the startup bindings table lists it:

```text
env.CHANNELS (OcreChannel)                                 Durable Object            local
```

### Watching the broadcasts

A browser is the usual client, but any WebSocket client shows the raw messages. This Node.js (22 or later) script prints what a channel receives:

```js
// listen.mjs: node listen.mjs ws://localhost:8787/realtime/messages
const ws = new WebSocket(process.argv[2]);
ws.onopen = () => console.log("open");
ws.onmessage = (event) => console.log("message:", event.data);
ws.onclose = (event) => console.log("close", event.code);
```

With `ocre dev` running, start it in one terminal, then create, edit and delete a message in another:

```sh
node listen.mjs ws://localhost:8787/realtime/messages
```

```sh
curl -s -o /dev/null -w "%{http_code}\n" -X POST http://localhost:8787/messages -d 'body=First'
curl -s -o /dev/null -w "%{http_code}\n" -X POST http://localhost:8787/messages/3 -d 'body=First, edited'
curl -s -o /dev/null -w "%{http_code}\n" -X POST http://localhost:8787/messages/3/delete
```

The listener prints (real run):

```text
open
message: <tbody hx-swap-oob="afterbegin:#messages"><tr id="message_3"><td>First</td><td><a href="/messages/3">Show</a> <a href="/messages/3/edit">Edit</a></td></tr></tbody>
message: <tr id="message_3"><td>First, edited</td><td><a href="/messages/3">Show</a> <a href="/messages/3/edit">Edit</a></td></tr>
message: <div id="message_3" hx-swap-oob="delete"></div>
```

## Messages and helpers

A message is text: usually HTML, JSON for non-htmx clients. For htmx, each top-level element of a message is swapped into the page element with the same `id`:

| Build it with | Message | Effect in the page |
|---|---|---|
| an element with an `id` | `<tr id="message_3">...</tr>` | Replaces the element with that id |
| `realtime::prepend(target, html)` | `<tbody hx-swap-oob="afterbegin:#messages">...</tbody>` | Inserts `html` at the start of `#target` |
| `realtime::append(target, html)` | `<ul hx-swap-oob="beforeend:#comments">...</ul>` | Inserts `html` at the end of `#target` |
| `realtime::update(target, html)` | `<div hx-swap-oob="innerHTML:#post_count">3 posts</div>` | Replaces the contents of `#target` |
| `realtime::remove(id)` | `<div id="post_3" hx-swap-oob="delete"></div>` | Removes `#id` |

`prepend`, `append` and `update` wrap the fragment in an element the browser can parse it in (a `<tr>` in a `<tbody>`, an `<li>` in a `<ul>`, likewise for other table parts, options and `<dt>`/`<dd>`, anything else in a `<div>`); htmx drops the wrapper. `target` and `id` are escaped; the HTML is sent as is, so render it with askama templates, which escape user content. Several swaps can go in one message: concatenate them.

The helpers are pure functions; nothing is sent until `broadcast`.

## Broadcasting from a handler

This page has a form that posts with htmx and a list subscribed to the `announcements` channel. The handler stores nothing: it renders the item and broadcasts it, and every open page, the sender's included, appends it. Add `"announcements" => {}` to `connect` in `src/realtime.rs` (see the next section) and register the module in `src/lib.rs`.

```rust,check
// src/announcements.rs
use askama::Template;
use axum::{
    Form, Router,
    extract::State,
    http::StatusCode,
    response::Html,
    routing::get,
};
use ocre::{Ctx, Result, Validator, realtime, render};
use serde::Deserialize;

#[derive(Template)]
#[template(
    source = r#"<!doctype html>
<script src="https://unpkg.com/htmx.org@2.0.4" crossorigin="anonymous"></script>
<script src="https://unpkg.com/htmx-ext-ws@2.0.4/dist/ws.js" crossorigin="anonymous"></script>
<h1>Announcements</h1>
<form hx-post="/announcements" hx-swap="none" hx-on::after-request="this.reset()">
  <input name="text" required> <button>Announce</button>
</form>
<div hx-ext="ws" ws-connect="/realtime/announcements">
  <ul id="announcements"></ul>
</div>"#,
    ext = "html"
)]
struct IndexView;

/// One announcement, escaped by askama like any page.
#[derive(Template)]
#[template(source = r#"<li id="announcement_{{ at }}">{{ text }}</li>"#, ext = "html")]
struct ItemView<'a> {
    at: i64,
    text: &'a str,
}

#[derive(Deserialize)]
struct AnnouncementForm {
    text: String,
}

pub fn routes() -> Router<Ctx> {
    Router::new().route("/announcements", get(index).post(create))
}

async fn index() -> Result<Html<String>> {
    render(&IndexView)
}

/// Sends the announcement to every open page, the sender's included. Nothing is stored.
async fn create(State(ctx): State<Ctx>, Form(form): Form<AnnouncementForm>) -> Result<StatusCode> {
    Validator::new().required("text", &form.text).finish()?;
    let item = render(&ItemView { at: ocre::now(), text: &form.text })?.0;
    // One Durable Object request, whatever the number of subscribers.
    realtime::broadcast(&ctx, "announcements", &realtime::append("announcements", &item)).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

A listener on `ws://localhost:8787/realtime/announcements` receives, for `curl -X POST http://localhost:8787/announcements -d 'text=Deploy at 5 pm <b>sharp</b>'` (answered `204`):

```text
message: <ul hx-swap-oob="beforeend:#announcements"><li id="announcement_1790656892">Deploy at 5 pm &#60;b&#62;sharp&#60;/b&#62;</li></ul>
```

Here the broadcast is the whole point of the request, so its error is returned (`?`). The generated controllers treat broadcasts as best effort instead: they `.ok()` the result, so a failure is only logged and the database write still succeeds. Either way, broadcast after the database write succeeded, and once per change, not once per row in a loop.

Every failure is logged as `[ocre realtime] broadcast to <channel> failed: ...` and returned as a 500 (`Error::Internal`): an invalid channel name, a missing `CHANNELS` binding (the message names the `cloudflare.config.ts` entries to add), or a channel object that cannot be reached, for example past the daily free-plan quota.

## Authorizing channels

`src/realtime.rs` is app code: `connect` decides who may open which channel before `upgrade.connect(&ctx, &channel)` hands the socket to the channel's object. The generated version lets everyone listen to the listed channels and answers 404 for the others. Browsers send the session cookie with the WebSocket handshake, so the session and the auth extractors work there.

This version keeps `messages` and `announcements` public and adds private per-user channels, `user:<id>`, that only that user may open:

```rust,check
// src/realtime.rs
use axum::{
    Router,
    extract::{Path, State},
    response::Response,
    routing::get,
};
use ocre::{Ctx, Error, Result, realtime::WebSocketUpgrade};

use crate::auth::OptionalUser;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/realtime/{channel}", get(connect)).merge(ocre::realtime::dev_routes())
}

/// Opens a WebSocket on `channel` for whoever may listen to it.
/// Public channels are listed by name; `user:<id>` is only for that user.
async fn connect(
    State(ctx): State<Ctx>,
    Path(channel): Path<String>,
    OptionalUser(user): OptionalUser,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    match channel.as_str() {
        // ocre:channels
        "messages" => {}
        "announcements" => {}
        private => {
            let Some(id) = private.strip_prefix("user:") else { return Err(Error::NotFound) };
            let user = user.ok_or(Error::Unauthorized)?;
            if id != user.id.to_string() {
                return Err(Error::Forbidden);
            }
        }
    }
    upgrade.connect(&ctx, &channel).await
}
```

`OptionalUser` keeps public channels open to visitors; taking `CurrentUser` instead would redirect every anonymous handshake to `/login`. Keep the `// ocre:channels` marker: later `--realtime` scaffolds add their channel under it.

Handshakes sent with curl (`-H Connection:Upgrade -H Upgrade:websocket -H Sec-WebSocket-Version:13 -H Sec-WebSocket-Key:dGhlIHNhbXBsZSBub25jZQ==`), with and without the session cookie of user 1, real run:

| Request | Status |
|---|---|
| `/realtime/announcements`, anonymous | `101 Switching Protocols` |
| `/realtime/nope`, anonymous | `404 Not Found` |
| `/realtime/user:1`, anonymous | `401 Unauthorized` |
| `/realtime/user:1`, signed in as user 1 | `101 Switching Protocols` |
| `/realtime/user:2`, signed in as user 1 | `403 Forbidden` |
| `/realtime/announcements` with `Sec-Fetch-Site: cross-site` | `403 Forbidden` |
| `/realtime/announcements` without `Upgrade: websocket` | `400` |

The cross-site refusal comes from `ocre::serve`, which checks WebSocket handshakes like unsafe form posts (see [Sessions, flash and security](security.md)). A request without the upgrade headers gets a 400 page: "Expected a WebSocket connection (`Upgrade: websocket`). Connect with htmx's ws extension or `new WebSocket(url)`."

Rules for channels:

- Names are 1 to 128 ASCII letters, digits, `_`, `-`, `.` or `:` (`posts`, `post:12`, `user:7`). `upgrade.connect` answers 400 for other names (a `connect` that lists its channels answers 404 first) and `broadcast` fails with a 500.
- Private data goes on a channel per user or per record, checked in `connect`; never on a shared channel. Anyone who may open a channel receives everything broadcast to it.
- A page subscribes to its private channel with the id from the session, e.g. `<div hx-ext="ws" ws-connect="/realtime/user:{{ user.id }}">`.
- Channel parameters (Rails' `params` of a subscription) are the route's: the channel name in the path, plus any query string read with axum's `Query` extractor in `connect`.

## Identifying subscribers and relaying client messages

Two builder methods on `WebSocketUpgrade`, called in `connect` before `connect(..)`:

- `upgrade.identified_by(id)` names the subscriber (Rails' `identified_by :current_user`), usually the user's id. The identity stays with the socket in the channel's object, even while it hibernates. At most 256 bytes.
- `upgrade.rebroadcast()` lets this client publish: every text message it sends (up to 16 KB) goes to the channel's other sockets as JSON, `{"from": "<identity>", "data": <message>}`. `from` is set by the server (`null` without `identified_by`), so a client cannot pretend to be someone else; `data` is the message parsed as JSON, or a string. No app code runs for relayed messages and they are never HTML swaps, so a client cannot inject markup into other pages. Use it for typing indicators, cursors or ephemeral chat between JavaScript clients; send anything that must be validated or stored to an ordinary route, which saves it and calls `broadcast`.

```rust,check
// src/chat.rs
use axum::{
    Router,
    extract::{Path, State},
    response::Response,
    routing::get,
};
use ocre::{Ctx, Error, Result, realtime::WebSocketUpgrade};

use crate::auth::OptionalUser;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/chat/{room}", get(join))
}

/// Signed-in users join `chat:<room>`; what each sends reaches the others as
/// `{"from": "<user id>", "data": ...}`.
async fn join(
    State(ctx): State<Ctx>,
    Path(room): Path<String>,
    OptionalUser(user): OptionalUser,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    let user = user.ok_or(Error::Unauthorized)?;
    upgrade.identified_by(user.id.to_string()).rebroadcast().connect(&ctx, &format!("chat:{room}")).await
}
```

In the page, `const ws = new WebSocket("/chat/lobby"); ws.send(JSON.stringify({ typing: true }))` reaches every other member as `{"from":"1","data":{"typing":true}}`. Broadcasts sent with `ocre::realtime::broadcast` go to every socket, publishers included.

Free plan: Cloudflare bills incoming WebSocket messages to a Durable Object at 20 messages per request; relaying them to the other sockets is free.

## Testing broadcasts

In `ocre dev`, `GET /ocre/dev/realtime/sent.json` lists the last 50 successful broadcasts of the Worker, oldest first, the way `/ocre/dev/mailers/sent.json` lists emails (Rails' `assert_broadcasts` and `assert_broadcast_on`):

```json
[{"id":1,"channel":"messages","message":"<tbody hx-swap-oob=\"afterbegin:#messages\">...</tbody>"}]
```

An end-to-end test creates a record, then checks that the list grew with the expected channel and HTML, without opening a WebSocket (see [Testing](testing.md)). The generated `src/realtime.rs` merges `ocre::realtime::dev_routes()`; deployed (release) builds answer 404 there. The message builders (`prepend`, `update`...) are pure functions, testable with plain unit tests.

## Broadcasting from a job

`broadcast` works anywhere a `Ctx` is available, including jobs, which is how long work reports back to the page that started it. After `ocre g job ImportFinished user_id:integer rows:integer`, edit the generated `perform`:

```rust,check
// src/jobs/import_finished.rs
use ocre::{Ctx, Result, realtime};
use serde::{Deserialize, Serialize};

/// The job's arguments, stored in the queue message as JSON.
#[derive(Debug, Serialize, Deserialize)]
pub struct ImportFinished {
    pub user_id: i64,
    pub rows: i64,
}

impl ImportFinished {
    /// Tells the user's open pages that the import is done: the element
    /// `id="import_status"` gets the new text. `Err` retries the job.
    pub async fn perform(self, ctx: &Ctx) -> Result<()> {
        let html = realtime::update("import_status", &format!("Import finished: {} rows", self.rows));
        realtime::broadcast(ctx, &format!("user:{}", self.user_id), &html).await
    }
}
```

The page shows `<p id="import_status">Importing...</p>` inside `<div hx-ext="ws" ws-connect="/realtime/user:{{ user.id }}">`. Returning the broadcast's error makes the queue retry the job; use `.ok()` when an update that nobody sees is not worth a retry. A job may run twice, so a repeated broadcast must be harmless (replacing contents, as here, is).

## Free-plan costs

Limits of the Workers Free plan (September 2026):

| Resource | Free plan | Realtime use |
|---|---|---|
| [Durable Object requests](https://developers.cloudflare.com/durable-objects/platform/pricing/) | 100,000 a day | 1 per connection (and reconnection), 1 per broadcast (even with no subscriber); incoming WebSocket messages count 1/20 (only clients connected with `rebroadcast()` have a reason to send any); messages to browsers are free |
| [Durable Object duration](https://developers.cloudflare.com/durable-objects/platform/pricing/) | 13,000 GB-s a day (128 MB objects: about 28 hours awake) | Only while handling a connection or a broadcast, a few milliseconds; hibernated sockets cost nothing |
| [Worker requests](https://developers.cloudflare.com/workers/platform/pricing/) | 100,000 a day | 1 per connection; a broadcast is a subrequest of the request that sends it |
| [Durable Object limits](https://developers.cloudflare.com/durable-objects/platform/limits/) | SQLite-backed classes only; 32,768 WebSockets per object | `new_sqlite_classes = ["OcreChannel"]`; one object per channel |

So the daily count is roughly: connections and reconnections of every open page, plus one per broadcast. Broadcasting per row in a loop, or many pages reconnecting often, is what exhausts the quota. The Durable Object request quota is shared with any other Durable Object use of the account.

## Deploying

`ocre deploy` needs no extra step: `cf deploy` creates the Durable Object namespace from the `OcreChannel` export. Keep the export as generated: removing or renaming it deletes the class and its objects.

Apps first deployed with an earlier Ocre declared the class with a wrangler `[[migrations]]` entry (tag `ocre-realtime-v1`). How cf maps the export onto such a Worker is not verified yet: see [Upgrading from wrangler.toml](upgrading.md#realtime-apps-deploy-to-a-preview-first) and deploy to a preview first.

## Without the scaffold, and in API-only apps

The scaffold only writes app code around three framework pieces, so any app, API-only included, can set them up by hand:

1. Turn on the feature in `Cargo.toml`: `ocre = { ..., features = ["realtime"] }` (see [Configuration](../reference/configuration.md#ocre-features)).
2. Add the `CHANNELS` binding and the `OcreChannel` export shown above to `cloudflare.config.ts`.
3. Add a `connect` route like `src/realtime.rs` above (any path; `WebSocketUpgrade` is the extractor) and merge it in `routes()`.
4. Broadcast from handlers or jobs. Non-htmx clients usually want JSON: `realtime::broadcast(&ctx, "orders", &ocre::serde_json::json!({"id": 12, "status": "paid"}).to_string())`. `WebSocketUpgrade` rejects a request without `Upgrade: websocket` with a 400 rendered as JSON in API-only apps.

A browser client without htmx is a plain `new WebSocket("wss://<host>/realtime/orders")` with an `onmessage` handler; clients do not need to send anything.

## Coming from Rails

| Action Cable | Ocre |
|---|---|
| `ApplicationCable::Connection`, `identified_by`, `reject_unauthorized_connection` | The `connect` handler: extractors, `identified_by`, `Err(Error::Forbidden)` |
| Connection and channel callbacks, `rescue_from` | Code before `upgrade.connect(..)` and its `Result`; the channel object runs no app code |
| Channel classes, `subscribed`, `stream_from`, `stream_for` | One channel name per stream (`post:12`), checked in `connect` |
| Channel `params` | The route's path and query string |
| Client actions (`perform`) | Ordinary routes (htmx `hx-post`) that `broadcast` |
| Rebroadcasting client data | `upgrade.rebroadcast()` |
| `ActionCable.server.broadcast`, `broadcast_to` | `ocre::realtime::broadcast(&ctx, channel, message)` |
| Subscription adapters (async, Redis, PostgreSQL, Solid Cable) | One: the `OcreChannel` Durable Object, no pub/sub server |
| Mount path, `action_cable_meta_tag`, allowed origins | The `connect` route's path, the `ws-connect` URL, the same-site check of `ocre::serve` |
| Standalone cable server, worker pool | Each channel's own Durable Object, apart from the request Worker; nothing to size |
| `assert_broadcasts`, `assert_broadcast_on` | `/ocre/dev/realtime/sent.json` in `ocre dev` |
| `createConsumer`, `subscriptions.create` | htmx's `ws` extension, or `new WebSocket(url)` |

## See also

- [Generators](../reference/generators.md#ocre-g-scaffold): `ocre g scaffold --realtime`.
- [Configuration](../reference/configuration.md#env-channels-and-exports-ocrechannel-durable-objects): the Durable Object entries.
- [Background jobs and schedules](jobs.md): enqueueing jobs that broadcast.
- [Authentication](authentication.md): `CurrentUser` and `OptionalUser`.
- [Sessions, flash and security](security.md): the cross-site check on handshakes.
- [Free-plan limits](../reference/limits.md) and [Cost model](../explanations/cost-model.md).
- [`ocre::realtime` rustdoc](/api/ocre/realtime/index.html).
