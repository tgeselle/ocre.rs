# Tutorial: a live Q&A app

This tutorial builds a live Q&A app with Ocre, step by step: hosts sign up and create events, anyone with an event's link asks questions and votes for them, and the list reorders live on every open screen over WebSockets. Along the way it covers generators, accounts, ownership checks, realtime channels, validations, request tests and a deploy to the Cloudflare Workers free plan.

## Before you start

- The tools from [Installation](installation.md): Rust through rustup with the `wasm32-unknown-unknown` target, Node.js 22 or newer, and the `ocre` CLI (`ocre --version` prints `ocre 0.2.0`).
- About 30 minutes. Nothing here needs a Cloudflare account until [Deploy](#deploy); the whole app runs locally.
- `curl`, to follow along from a terminal, and a browser for the live part: every page is a normal HTML page at `http://localhost:8787`.

The commands and outputs on this page come from a real run, except [Deploy](#deploy), which is described from the CLI's code. Outputs that change from run to run (dates, random ids, cookie values) will differ on your machine. The finished app is also a starter: `ocre new qa --starter qa` creates it in one command.

## Create the app

```sh
ocre new qa --yes
cd qa
```

```text
  create  qa/Cargo.toml
  create  qa/cloudflare.config.ts
  create  qa/wrangler.config.ts
  create  qa/package.json
  create  qa/tsconfig.json
  create  qa/rust-toolchain.toml
  create  qa/rustfmt.toml
  create  qa/.gitignore
  create  qa/AGENTS.md
  create  qa/migrations/.gitkeep
  create  qa/public/robots.txt
  create  qa/tests/app.rs
  create  qa/src/lib.rs
  create  qa/templates/layout.html
  create  qa/templates/home.html
  create  qa/templates/error.html
  create  qa/.dev.vars
  npm install (cf 1.0.0-beta.5, wrangler 4.144.0)

Next:
  cd qa
  ocre dev
  ocre deploy
```

`--yes` skips the guided setup and keeps its defaults: a full-stack app (HTML pages), the empty starter, no Git repository, no Cloudflare login. `ocre new` then runs `npm install` in the app, which installs Cloudflare's `cf` CLI (with the `wrangler` it delegates builds to, and `typescript`) at the versions pinned in `package.json`; pass `--no-install` to skip it when offline. Without `--yes`, in a terminal, `ocre new qa` asks those questions instead (see [Installation](installation.md#create-an-app-with-the-guided-setup)).

The app is a Rust crate compiled to WebAssembly and run as one Cloudflare Worker. `src/lib.rs` is the entry point and the router; generators add modules under its `// ocre:modules` line and routes under `// ocre:routes`. `templates/layout.html` loads htmx and sets `hx-boost="true"` on `<body>`, so links and forms swap the page body instead of reloading it, and every page still works without JavaScript (see [htmx](../guides/htmx.md)). `AGENTS.md` summarizes the app's conventions and commands for AI agents.

## Hosts: accounts

Hosts sign up to create events; the audience never needs an account. `ocre g auth` writes sign-up, login, logout, password reset and magic links into the app:

```sh
ocre g auth
```

```text
  create  migrations/0001_create_users.sql
  create  migrations/0002_create_auth_tokens.sql
  create  migrations/0003_create_api_keys.sql
  create  src/models/mod.rs
  create  src/models/user.rs
  create  src/models/api_key.rs
  create  src/models/auth_token.rs
  create  src/auth_api.rs
  create  src/auth.rs
  create  src/registrations.rs
  create  src/sessions.rs
  create  src/passwords.rs
  create  src/confirmations.rs
  create  templates/auth/signup.html
  create  templates/auth/login.html
  create  templates/auth/account.html
  create  templates/auth/magic_link_new.html
  create  templates/auth/magic_link_show.html
  create  templates/auth/password_new.html
  create  templates/auth/password_edit.html
  create  templates/auth/confirmation_show.html
  update  src/lib.rs
  update  cloudflare.config.ts

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/signup
```

The code is the app's own: `src/auth.rs` holds the extractors handlers take, `CurrentUser` (signed in, otherwise redirected to `/login`) and `OptionalUser` (signed in or not). See [Authentication](../guides/authentication.md) for every file.

## Events

An event belongs to a host and has a public link. Generate it with a `user` reference and a random public id:

```sh
ocre g scaffold Event name:string public_id:token user:references
```

```text
  create  migrations/0004_create_events.sql
  create  src/models/event.rs
  create  tests/factories/mod.rs
  create  tests/factories/event.rs
  create  src/events.rs
  create  templates/events/index.html
  create  templates/events/show.html
  create  templates/events/new.html
  create  templates/events/edit.html
  create  templates/events/_form.html
  create  tests/events.rs
  update  src/models/user.rs
  update  src/models/mod.rs
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/events
```

- `public_id:token` gives each event 22 random URL-safe characters, set by `create`, and the generated pages use it in URLs instead of the integer id: `/events/c1wWvvdHUcKW080CDwgf6A`, which nobody can guess or enumerate. That link is what a host shares.
- `user:references` adds `user_id` with a foreign key to `users`, `event.user(&ctx)` on the event and `user.events(&ctx, page)` on the user.

## Questions, live

Questions belong to an event. `--realtime` makes the generated pages live: every create, edit and delete is pushed to every open list over a WebSocket.

```sh
ocre g scaffold Question event:references body:text votes:integer answered:boolean --realtime
```

```text
  create  migrations/0005_create_questions.sql
  create  src/models/question.rs
  create  tests/factories/question.rs
  create  src/questions.rs
  create  templates/questions/index.html
  create  templates/questions/show.html
  create  templates/questions/new.html
  create  templates/questions/edit.html
  create  templates/questions/_form.html
  create  templates/questions/_row.html
  create  tests/questions.rs
  create  src/realtime.rs
  update  src/models/event.rs
  update  src/models/mod.rs
  update  tests/factories/mod.rs
  update  src/lib.rs
  update  Cargo.toml
  update  cloudflare.config.ts
  update  templates/layout.html

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/questions
  open http://localhost:8787/questions in a second window, then create a question
```

The first `--realtime` scaffold sets up the pieces every live page needs:

| File | What changed |
|---|---|
| `Cargo.toml` | Ocre's `realtime` feature |
| `cloudflare.config.ts` | The `CHANNELS` Durable Object binding and the `OcreChannel` export: one Durable Object per channel holds the browsers' WebSockets, and hibernates between messages so idle connections cost nothing |
| `src/realtime.rs` | `GET /realtime/{channel}`, where browsers connect, and `connect`, which decides who may listen to which channel |
| `templates/layout.html` | htmx's WebSocket extension, after htmx |
| `src/questions.rs` | After each create, update and delete, the handler broadcasts the changed row on the `questions` channel |

Apply the five migrations and start the app:

```sh
ocre migrate
ocre dev
```

`ocre dev` builds the Worker and serves it at `http://localhost:8787` with a local D1 database and the Durable Objects running in workerd, Cloudflare's runtime.

Try the generated pages: open `http://localhost:8787/signup` and create an account, then `http://localhost:8787/events/new` and create an event, then open `http://localhost:8787/questions` in two windows and create a question (event 1) in one of them: it appears in the other without a reload. From a terminal, with the same steps done by curl:

```sh
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" -c jar.txt http://localhost:8787/signup -d 'email=ada@example.com&password=correct horse'
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" http://localhost:8787/events -d 'name=Friday all-hands&user_id=1'
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" http://localhost:8787/questions -d 'event_id=1&body=Will there be pizza?&votes=0'
curl -s http://localhost:8787/ocre/dev/realtime/sent.json
```

```text
303 http://localhost:8787/
303 http://localhost:8787/events/1SPmf4grPih32uDXg8GP-g
303 http://localhost:8787/questions/1
[{"id":1,"channel":"questions","message":"<tbody hx-swap-oob=\"afterbegin:#questions\"><tr id=\"question_1\"><td>1</td><td>Will there be pizza?</td><td>0</td><td>false</td><td><a href=\"/questions/1\">Show</a> <a href=\"/questions/1/edit\">Edit</a></td></tr></tbody>"}]
```

`/ocre/dev/realtime/sent.json` lists the recent broadcasts (in `ocre dev` only; deployed apps answer 404). The message is HTML: htmx's WebSocket extension reads its `hx-swap-oob` attribute and inserts the row at the top of the element with id `questions` in every open page.

Two things are wrong with these generated pages for a Q&A app, and the next sections fix them. The second request created an event for user 1 without being signed in, because the form takes `user_id` like any other field. And every question of every event lands on one page, `/questions`, with edit buttons for everyone.

## Events belong to their host

The host of an event is the signed-in user, never a form field. In `src/events.rs`, the form keeps only the name, and `to_new` takes the host's id:

```rust
/// What the new and edit forms submit. The host is the signed-in user, never
/// a form field.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EventForm {
    pub name: String,
}

impl EventForm {
    fn from_record(record: &Event) -> Self {
        Self { name: record.name.clone() }
    }

    /// Runs the model's checks, so the form shows every error at once
    /// (database checks run in `create`).
    fn to_new(&self, user_id: i64) -> Result<NewEvent> {
        let new = NewEvent { name: self.name.clone(), user_id };
        new.validate().finish()?;
        Ok(new)
    }

    fn to_changes(&self) -> Result<EventChanges> {
        let changes = EventChanges { name: Some(self.name.clone()), user_id: None };
        changes.validate().finish()?;
        Ok(changes)
    }
}
```

Remove the `User` field from `templates/events/_form.html`, so it only has the name:

```html
{% if !errors.is_empty() %}
  <ul class="errors">
    {% for error in errors %}<li>{{ error.full_message() }}</li>{% endfor %}
  </ul>
{% endif %}
  <label>Name <input name="name" value="{{ form.name }}" required placeholder="Friday all-hands"></label>
```

The handlers that create or change an event take `CurrentUser`, and a small helper refuses other users with a 403:

```rust
/// The event `id`, when `user` hosts it: other users get a 403.
async fn hosted(ctx: &Ctx, user: &User, id: i64) -> Result<Event> {
    let record = event::find(ctx, id).await?.or_404()?;
    if record.user_id != user.id {
        return Err(Error::Forbidden);
    }
    Ok(record)
}

/// The events the signed-in user hosts, newest first.
async fn index(State(ctx): State<Ctx>, CurrentUser(user): CurrentUser, flash: Flash) -> Result<Html<String>> {
    render(&IndexView { flash, events: event::for_users(&ctx, &[user.id]).await? })
}

async fn new(CurrentUser(_): CurrentUser) -> Result<Html<String>> {
    render(&NewView { form: EventForm::default(), errors: vec![] })
}

async fn create(
    State(ctx): State<Ctx>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Form(form): Form<EventForm>,
) -> Result<Response> {
    let created = match form.to_new(user.id) {
        Ok(new) => event::create(&ctx, new).await,
        Err(err) => Err(err),
    };
    match created {
        Ok(record) => {
            session.flash("notice", "Your event is ready: share its link.")?;
            Ok(Redirect::to(&paths::show(&record.public_id)).into_response())
        }
        Err(Error::Invalid(errors)) => {
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&NewView { form, errors })?).into_response())
        }
        Err(err) => Err(err),
    }
}
```

`edit`, `update` and `delete` take `CurrentUser(user): CurrentUser` too and start with `hosted(&ctx, &user, id).await?;`. The `use` lines gain `crate::auth::{CurrentUser, OptionalUser}` and `crate::models::user::User`. The complete file is `src/events.rs` of the qa starter.

`IndexView` loses its `page` field: `event::for_users`, generated by `user:references`, returns the host's events without pagination. `templates/events/index.html` becomes the host's list:

```html
{% extends "layout.html" %}

{% block title %}Your events{% endblock %}

{% block content %}
<h1>Your events</h1>
{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
{% if let Some(alert) = flash.alert() %}<p class="alert">{{ alert }}</p>{% endif %}
<p><a class="button" href="{{ paths::new() }}">New event</a></p>

{% if events.is_empty() %}
<p class="empty">No events yet: create one, then share its link with your audience.</p>
{% else %}
<ul class="events">
  {% for event in events %}
  <li><a href="{{ paths::show(event.public_id) }}">{{ event.name }}</a> <span class="muted">created {{ event.created_at }}</span></li>
  {% endfor %}
</ul>
{% endif %}
{% endblock %}
```

## The room

An event's page becomes its room: the event's name, the link to share, a form to ask, and the live list of its questions, most voted first. Questions get their own routes under the event, and the generated question pages go away:

```sh
rm templates/questions/index.html templates/questions/show.html templates/questions/new.html \
   templates/questions/edit.html templates/questions/_form.html templates/questions/_row.html
```

Two queries go in the model, `src/models/question.rs`, after `for_events`:

```rust
/// The questions of an event as the room shows them: open ones first, most
/// voted first (oldest first on a tie), then answered ones. At most 200.
pub async fn for_event(ctx: &Ctx, event_id: i64) -> Result<Vec<Question>> {
    query()
        .eq("event_id", event_id)
        .order_asc("answered")
        .order_desc("votes")
        .order_asc("id")
        .limit(200)
        .all(&ctx.db()?)
        .await
}

/// Adds one vote. A single UPDATE, so votes arriving at the same moment are
/// all counted. `None` when there is no question with this id.
pub async fn vote(ctx: &Ctx, id: i64) -> Result<Option<Question>> {
    let sql = "UPDATE questions SET votes = votes + 1, updated_at = datetime('now') WHERE id = ?1 RETURNING *";
    ctx.db()?.first(sql, params![id]).await
}
```

`src/questions.rs` is rewritten for the room. Replace it with:

```rust
//! Questions in an event's room: the audience asks and votes, the host marks
//! questions answered or deletes them. After each change the event's list is
//! rendered again and broadcast (channels in src/realtime.rs), so it reorders
//! live in every open room.

use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, State},
    http::{StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::post,
};
use ocre::{Ctx, Error, Flash, Htmx, OptionExt, Result, Session, realtime, render};
use serde::Deserialize;

use crate::auth::{CurrentUser, OptionalUser};
use crate::events;
use crate::models::event::{self, Event};
use crate::models::question::{self, NewQuestion, Question, QuestionChanges};
use crate::models::user::User;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/events/{id}/questions", post(ask))
        .route("/questions/{id}/vote", post(vote))
        .route("/questions/{id}/answer", post(answer))
        .route("/questions/{id}/delete", post(delete))
}

pub mod paths {
    use std::fmt::Display;

    /// `id` is the event's public id.
    pub fn ask(id: impl Display) -> String {
        format!("/events/{id}/questions")
    }

    pub fn vote(id: impl Display) -> String {
        format!("/questions/{id}/vote")
    }

    pub fn answer(id: impl Display) -> String {
        format!("/questions/{id}/answer")
    }

    pub fn delete(id: impl Display) -> String {
        format!("/questions/{id}/delete")
    }
}

/// What the ask form submits.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct QuestionForm {
    pub body: String,
}

/// The question list of a room, also sent to open rooms on every change.
/// `host` adds the moderation buttons.
#[derive(Template)]
#[template(path = "questions/_list.html")]
struct ListView<'a> {
    questions: &'a [Question],
    host: bool,
}

/// The channel every visitor of the room listens to.
pub fn channel(event: &Event) -> String {
    format!("event:{}", event.public_id)
}

/// The host's channel: the same list, with moderation buttons.
pub fn host_channel(event: &Event) -> String {
    format!("host:{}", event.public_id)
}

/// Sends the event's current list to both channels. Best effort: a failed
/// broadcast is logged by Ocre and never fails the request.
async fn broadcast(ctx: &Ctx, event: &Event) -> Result<()> {
    let questions = question::for_event(ctx, event.id).await?;
    let audience = render(&ListView { questions: &questions, host: false })?.0;
    let host = render(&ListView { questions: &questions, host: true })?.0;
    realtime::broadcast(ctx, &channel(event), &realtime::update("questions", &audience)).await.ok();
    realtime::broadcast(ctx, &host_channel(event), &realtime::update("questions", &host)).await.ok();
    Ok(())
}

async fn ask(
    State(ctx): State<Ctx>,
    session: Session,
    flash: Flash,
    OptionalUser(user): OptionalUser,
    uri: Uri,
    Path(key): Path<String>,
    Form(form): Form<QuestionForm>,
) -> Result<Response> {
    let event = event::find_by_public_id(&ctx, &key).await?.or_404()?;
    let new = NewQuestion { event_id: event.id, body: form.body.trim().to_owned(), votes: 0, answered: false };
    match question::create(&ctx, new).await {
        Ok(_) => {
            broadcast(&ctx, &event).await?;
            session.flash("notice", "Your question is in.")?;
            Ok(Redirect::to(&events::paths::show(&key)).into_response())
        }
        Err(Error::Invalid(errors)) => {
            let room = events::room(&ctx, flash, &uri, event, user, form, errors).await?;
            Ok((StatusCode::UNPROCESSABLE_ENTITY, room).into_response())
        }
        Err(err) => Err(err),
    }
}

/// Session key: the questions this browser voted for.
const VOTED: &str = "voted";

/// One vote per question and browser; voting again changes nothing.
async fn vote(State(ctx): State<Ctx>, session: Session, Htmx(is_htmx): Htmx, Path(id): Path<i64>) -> Result<Response> {
    let record = question::find(&ctx, id).await?.or_404()?;
    let event = record.event(&ctx).await?.or_404()?;
    let mut voted: Vec<i64> = session.get(VOTED)?.unwrap_or_default();
    if !voted.contains(&id) {
        question::vote(&ctx, id).await?;
        // The session is a cookie (4 KB at most): remember the last 200 votes.
        if voted.len() >= 200 {
            voted.remove(0);
        }
        voted.push(id);
        session.insert(VOTED, &voted)?;
        broadcast(&ctx, &event).await?;
    }
    Ok(done(is_htmx, &event))
}

/// Marks a question answered, or open again.
async fn answer(
    State(ctx): State<Ctx>,
    CurrentUser(user): CurrentUser,
    Htmx(is_htmx): Htmx,
    Path(id): Path<i64>,
) -> Result<Response> {
    let (record, event) = hosted(&ctx, &user, id).await?;
    let changes = QuestionChanges { answered: Some(!record.answered), ..Default::default() };
    question::update(&ctx, id, changes).await?;
    broadcast(&ctx, &event).await?;
    Ok(done(is_htmx, &event))
}

async fn delete(
    State(ctx): State<Ctx>,
    CurrentUser(user): CurrentUser,
    Htmx(is_htmx): Htmx,
    Path(id): Path<i64>,
) -> Result<Response> {
    let (_, event) = hosted(&ctx, &user, id).await?;
    question::delete(&ctx, id).await?;
    broadcast(&ctx, &event).await?;
    Ok(done(is_htmx, &event))
}

/// The question `id` and its event, when `user` hosts that event.
async fn hosted(ctx: &Ctx, user: &User, id: i64) -> Result<(Question, Event)> {
    let record = question::find(ctx, id).await?.or_404()?;
    let event = record.event(ctx).await?.or_404()?;
    if event.user_id != user.id {
        return Err(Error::Forbidden);
    }
    Ok((record, event))
}

/// htmx buttons get a 204 (the broadcast updates the page); a plain form
/// post goes back to the room.
fn done(is_htmx: bool, event: &Event) -> Response {
    if is_htmx {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Redirect::to(&events::paths::show(&event.public_id)).into_response()
    }
}
```

What it does:

- **One list, rendered on the server.** After any change, `broadcast` loads the event's questions in their room order and sends the whole list, rendered by `templates/questions/_list.html`, wrapped by `realtime::update("questions", ...)`: htmx replaces the contents of `#questions` in every open room. Sending the list rather than one row is what makes it reorder live when votes change the ranking. It costs one D1 query and two Durable Object requests per change; the messages measured about 350 bytes per question for the audience and 800 for the host (with its buttons), so about 70 KB and 160 KB at the 200-question cap.
- **Two channels.** The audience and the host see the same list, except that the host's has "Mark answered" and "Delete" buttons. A broadcast reaches everyone on a channel, so each version goes to its own channel: `event:<public id>` and `host:<public id>`.
- **One vote per browser.** The session (a signed, encrypted cookie that every visitor has) remembers the questions this browser voted for. An anonymous visitor who clears their cookies can vote again, as in most live Q&A tools; votes that must be unique per person need accounts.
- **htmx buttons.** The vote and moderation buttons post with htmx and get a `204 No Content`: the layout tells htmx not to swap anything for a 204, and the broadcast updates the page, the voter's included. Without JavaScript, the same forms post normally and the handler redirects back to the room.
- **Moderation is the host's.** `answer` and `delete` take `CurrentUser` (visitors are sent to `/login`) and `hosted` answers 403 to other users.

The room page is the event's `show`. In `src/events.rs`, `ShowView` gets the room's data and `show` renders it through `room`, which `ask` also calls to show the form's errors (the file's `use` lines gain `http::Uri`, `crate::models::question::{self, Question}` and `crate::questions::{self, QuestionForm}`):

```rust
/// The room: the event, the ask form and the live question list.
#[derive(Template)]
#[template(path = "events/show.html")]
struct ShowView {
    flash: Flash,
    event: Event,
    /// The signed-in user hosts this event: moderation buttons and the
    /// host's channel.
    host: bool,
    /// The full link to share with the audience.
    link: String,
    questions: Vec<Question>,
    form: QuestionForm,
    errors: Vec<FieldError>,
}

impl ShowView {
    /// The channel this page listens to (src/realtime.rs).
    fn channel(&self) -> String {
        if self.host { questions::host_channel(&self.event) } else { questions::channel(&self.event) }
    }
}

async fn show(
    State(ctx): State<Ctx>,
    flash: Flash,
    OptionalUser(user): OptionalUser,
    uri: Uri,
    Id(id, ..): Id,
) -> Result<Html<String>> {
    let record = event::find(&ctx, id).await?.or_404()?;
    room(&ctx, flash, &uri, record, user, QuestionForm::default(), vec![]).await
}

/// Renders the room of `event`; the ask form shows `form` and `errors`.
pub async fn room(
    ctx: &Ctx,
    flash: Flash,
    uri: &Uri,
    event: Event,
    user: Option<User>,
    form: QuestionForm,
    errors: Vec<FieldError>,
) -> Result<Html<String>> {
    let host = user.is_some_and(|user| user.id == event.user_id);
    let link = format!("{}{}", crate::auth::origin(uri), paths::show(&event.public_id));
    let questions = question::for_event(ctx, event.id).await?;
    render(&ShowView { flash, event, host, link, questions, form, errors })
}
```

`templates/events/show.html`:

```html
{% extends "layout.html" %}

{% block title %}{{ event.name }}{% endblock %}

{% block content %}
{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
{% if let Some(alert) = flash.alert() %}<p class="alert">{{ alert }}</p>{% endif %}

<section class="room-head">
  <p class="eyebrow">Live Q&amp;A</p>
  <h1>{{ event.name }}</h1>
  <p class="share">Share this link: <code>{{ link }}</code></p>
  {% if host %}
  <div class="host-tools">
    <p>You host this event. <a href="{{ paths::edit(event.public_id) }}">Rename</a> · <a href="{{ paths::index() }}">Your events</a></p>
    <form action="{{ paths::delete(event.public_id) }}" method="post" hx-confirm="Delete this event and its questions?">
      <button class="link danger" type="submit">Delete event</button>
    </form>
  </div>
  {% endif %}
</section>

<form class="ask" action="{{ crate::questions::paths::ask(event.public_id) }}" method="post">
  {% if !errors.is_empty() %}
  <ul class="errors">
    {% for error in errors %}<li>{{ error.full_message() }}</li>{% endfor %}
  </ul>
  {% endif %}
  <label>Your question <textarea name="body" rows="2" maxlength="280" required placeholder="Ask anything…">{{ form.body }}</textarea></label>
  <button type="submit">Ask</button>
</form>

{# Live updates: htmx's WebSocket extension (loaded by layout.html) swaps in the list every change broadcasts (src/questions.rs, channels in src/realtime.rs). #}
<div hx-ext="ws" ws-connect="/realtime/{{ self.channel() }}">
  <div id="questions">{% include "questions/_list.html" %}</div>
</div>
{% endblock %}
```

And the list both the page and the broadcasts use, `templates/questions/_list.html`:

```html
{# The room's question list: rendered in the page and broadcast to open rooms on every change (src/questions.rs). `host` adds the moderation buttons. #}
{% if questions.is_empty() %}
<p class="empty">No questions yet. Be the first to ask.</p>
{% else %}
<ol class="questions">
  {% for question in questions %}
  {% let vote = crate::questions::paths::vote(question.id) %}
  <li id="question_{{ question.id }}" class="question{% if question.answered %} answered{% endif %}">
    <form action="{{ vote }}" method="post" hx-post="{{ vote }}" hx-swap="none">
      <button class="vote" type="submit" title="Vote for this question"{% if question.answered %} disabled{% endif %}>▲<span>{{ question.votes }}</span></button>
    </form>
    <p class="body">{{ question.body }}</p>
    {% if question.answered %}<span class="badge">Answered</span>{% endif %}
    {% if host %}
    {% let answer = crate::questions::paths::answer(question.id) %}
    {% let delete = crate::questions::paths::delete(question.id) %}
    <div class="moderate">
      <form action="{{ answer }}" method="post" hx-post="{{ answer }}" hx-swap="none">
        <button class="link" type="submit">{% if question.answered %}Reopen{% else %}Mark answered{% endif %}</button>
      </form>
      <form action="{{ delete }}" method="post" hx-post="{{ delete }}" hx-swap="none" hx-confirm="Delete this question?">
        <button class="link danger" type="submit">Delete</button>
      </form>
    </div>
    {% endif %}
  </li>
  {% endfor %}
</ol>
{% endif %}
```

askama escapes `{{ question.body }}`, so a question containing HTML shows as text, in the page and in broadcasts. The starter's `templates/events/index.html` (the host's events), `templates/home.html` (the landing page) and `templates/layout.html` (navigation and styles) are plain HTML; copy them from `ocre new demo --starter qa` or write your own.

## Who may listen

`src/realtime.rs` decides who may open which channel. The room's channels are named after the event's public id, so knowing the link is what lets a visitor in; the host's channel also needs the host's session. Replace `connect` (and add `OptionExt` to the `ocre` imports, with `use crate::auth::OptionalUser;` and `use crate::models::event;`):

```rust
/// Opens a WebSocket on `channel` for whoever may listen to it:
/// `event:<public id>`, an event's room, is open to everyone with the link;
/// `host:<public id>`, the same list with moderation buttons, only to the
/// event's host. Unknown channels and events are a 404.
async fn connect(
    State(ctx): State<Ctx>,
    Path(channel): Path<String>,
    OptionalUser(user): OptionalUser,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    match channel.as_str() {
        // ocre:channels
        name => {
            let (kind, key) = name.split_once(':').or_404()?;
            let record = event::find_by_public_id(&ctx, key).await?.or_404()?;
            match kind {
                "event" => {}
                "host" => {
                    let user = user.ok_or(Error::Unauthorized)?;
                    if user.id != record.user_id {
                        return Err(Error::Forbidden);
                    }
                }
                _ => return Err(Error::NotFound),
            }
        }
    }
    upgrade.connect(&ctx, &channel).await
}
```

Browsers send the session cookie with the WebSocket handshake, so `OptionalUser` works there like in any handler. Keep the `// ocre:channels` line: later `--realtime` scaffolds add their channel under it, before the catch-all arm.

## Try the room

With `ocre dev` running (it rebuilds on every change), sign in as the host, create an event, then ask and vote as a visitor with another cookie jar:

```sh
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" -c host.txt -b host.txt http://localhost:8787/login -d 'email=ada@example.com&password=correct horse'
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" -c host.txt -b host.txt http://localhost:8787/events -d 'name=Friday all-hands'
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" http://localhost:8787/events -d 'name=Sneaky'
```

```text
303 http://localhost:8787/
303 http://localhost:8787/events/c1wWvvdHUcKW080CDwgf6A
303 http://localhost:8787/login
```

The visitor without an account is sent to the login page. Now the audience, with the event's public id from the redirect:

```sh
EVENT=c1wWvvdHUcKW080CDwgf6A
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" -c guest.txt -b guest.txt http://localhost:8787/events/$EVENT/questions -d 'body=Will there be pizza?'
curl -s -o /dev/null -w "%{http_code}\n" -c guest.txt -b guest.txt http://localhost:8787/events/$EVENT/questions -d 'body=When is the next release?'
curl -s -o /dev/null -w "%{http_code}\n" -c guest.txt -b guest.txt -H 'HX-Request: true' -X POST http://localhost:8787/questions/3/vote
curl -s -o /dev/null -w "%{http_code}\n" -c guest.txt -b guest.txt -H 'HX-Request: true' -X POST http://localhost:8787/questions/3/vote
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" -b guest.txt -X POST http://localhost:8787/questions/2/answer
curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" -b host.txt -X POST http://localhost:8787/questions/2/answer
```

```text
303 http://localhost:8787/events/c1wWvvdHUcKW080CDwgf6A
303
204
204
303 http://localhost:8787/login
303 http://localhost:8787/events/c1wWvvdHUcKW080CDwgf6A
```

`ocre sql` runs a query on the local database:

```sh
ocre sql "SELECT id, event_id, body, votes, answered FROM questions"
```

```text
id | event_id | body                      | votes | answered
---+----------+---------------------------+-------+---------
1  | 1        | Will there be pizza?      | 0     | 0
2  | 2        | Will there be pizza?      | 0     | 1
3  | 2        | When is the next release? | 1     | 0
(3 rows)
```

Question 1 is the one asked on the generated `/questions` page earlier. The second vote of the same browser changed nothing, and only the host's request marked question 2 answered. In a browser, open the event's link in two windows (one signed in as the host, one private window): a question asked in one appears in the other, and a vote moves it up on both.

## Validate questions

A question needs at least 3 characters, and at most 280 so the room stays readable. In `src/models/question.rs`, `NewQuestion::validate` gets two rules:

```rust
impl NewQuestion {
    /// Checks that need no database; `create` adds uniqueness and references.
    pub fn validate(&self) -> Validator {
        let mut v = Validator::new();
        v.required("body", &self.body);
        v.min_length("body", &self.body, 3);
        v.max_length("body", &self.body, 280);
        v.safe_integer("votes", self.votes);
        v
    }
}
```

`question::create` runs it, so every way of creating a question gets the rules. `ask` turns the error into a 422 that shows the room again with the message above the form:

```sh
curl -s -c guest.txt -b guest.txt http://localhost:8787/events/$EVENT/questions -d 'body=hi' -w "%{http_code}\n" | grep -E "<li>Body|^4"
```

```text
    <li>Body is too short (minimum is 3 characters)</li>
422
```

See [Validations](../guides/validations.md) for every rule.

## Test it

Request tests send HTTP requests to the app running in workerd, with a fresh local database. Replace the generated `tests/events.rs` and `tests/questions.rs`, which tested the CRUD pages, with tests of the room. `tests/questions.rs`, shortened to its helpers and two tests:

```rust
use ocre::testing::{Client, sequence, sql};

/// A signed-up host and the public id of their new event.
fn host_with_event() -> (Client, String) {
    let mut client = Client::new();
    // Unique across test files, which share the test database.
    let email = format!("host{}-{}@example.com", std::process::id(), sequence());
    client.post("/signup", &[("email", email.as_str()), ("password", "correct horse")]).assert_status(303);
    let created = client.post("/events", &[("name", "Q&A")]);
    let location = created.assert_status(303).location().unwrap_or_default().to_owned();
    (client, location.trim_start_matches("/events/").to_owned())
}

/// `client` asks `body` in the room `event`; returns the question's id.
fn ask(client: &mut Client, event: &str, body: &str) -> i64 {
    client.post(&format!("/events/{event}/questions"), &[("body", body)]).assert_redirect_to(&format!("/events/{event}"));
    let rows = sql(&format!("SELECT id FROM questions WHERE body = {}", ocre::testing::quote(body)));
    rows[0]["id"].as_i64().expect("the question is saved")
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn a_question_appears_live_in_every_room() {
    let (_, event) = host_with_event();
    let mut visitor = Client::new();
    ask(&mut visitor, &event, "Will there be pizza?");
    visitor.get(&format!("/events/{event}")).assert_contains("Will there be pizza?");
    let sent = visitor.broadcasts();
    let audience = sent.iter().rev().find(|b| b.channel == format!("event:{event}")).expect("sent to the room");
    assert!(audience.message.contains("Will there be pizza?"), "{}", audience.message);
    assert!(!audience.message.contains("Mark answered"), "no moderation for the audience");
    let host = sent.iter().rev().find(|b| b.channel == format!("host:{event}")).expect("sent to the host");
    assert!(host.message.contains("Mark answered"), "{}", host.message);
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn each_browser_votes_once_per_question() {
    let (_, event) = host_with_event();
    let mut first = Client::new().htmx();
    let id = ask(&mut first, &event, "Can we get the slides?");
    first.post(&format!("/questions/{id}/vote"), &()).assert_status(204);
    first.post(&format!("/questions/{id}/vote"), &()).assert_status(204);
    assert_eq!(votes(id), 1);
    Client::new().htmx().post(&format!("/questions/{id}/vote"), &()).assert_status(204);
    assert_eq!(votes(id), 2);
}
```

Each `Client` keeps its own cookies, like a browser; `.htmx()` adds the `HX-Request` header; `broadcasts()` returns what the app sent to its channels. The starter's test files also check validation, ordering by votes, moderation rights, the host's pages and that visitors need no account. Run them:

```sh
ocre test --e2e
```

The command checks the build for `wasm32-unknown-unknown`, migrates a separate test database, starts the app once and runs every test file (12 request tests here); cargo's output ends with:

```text
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.77s
```

See [Testing an Ocre app](../guides/testing.md) for unit tests, factories and browser tests.

## List the routes

`ocre routes` reads the routes from `src/lib.rs` and the modules it merges, without building. A filter keeps the routes whose method, path or handler contains it; here, the app's own routes without the auth pages:

```sh
ocre routes
```

```text
METHOD  PATH                    HANDLER
GET     /                       home
GET     /events                 events::index
POST    /events                 events::create
GET     /events/new             events::new
GET     /events/{id}            events::show
POST    /events/{id}            events::update
POST    /events/{id}/delete     events::delete
GET     /events/{id}/edit       events::edit
POST    /events/{id}/questions  questions::ask
POST    /questions/{id}/answer  questions::answer
POST    /questions/{id}/delete  questions::delete
POST    /questions/{id}/vote    questions::vote
GET     /realtime/{channel}     realtime::connect
GET     /up                     up
```

The full list also has the account routes of `ocre g auth` (`/signup`, `/login`, `/logout`, `/account`, `/passwords/...`, `/magic_link/...`, `/confirmations/...`, `/api/auth/...`).

## Deploy

Deploying needs a Cloudflare account (the free plan is enough) and a login of Cloudflare's `cf` CLI. The commands in this section were not run for this page: the behavior below is described from the CLI's code.

### Log in

```sh
ocre login
```

If cf already has a login (or `CLOUDFLARE_API_TOKEN` is set), nothing opens. Otherwise it runs `cf auth login`, which opens the browser to approve access. The command then prints `Logged in to Cloudflare as <email>`. A login with several accounts needs `accountId: "...",` at the top of `cloudflare.config.ts` (`ocre new --account-id` writes it). cf also sends anonymous usage telemetry by default; `npx cf cli telemetry disable` turns it off.

### Email in production

Sign-up, login and the room send no email and work without it. The magic-link and password-reset forms do: in production `.dev.vars` does not apply, and with `MAIL_ADAPTER` unset they answer 500 and the log names the fix. To turn them on, pick an adapter; on the free plan, [Resend](https://resend.com/docs/knowledge-base/account-quotas-and-limits) sends to any recipient (free: 100 emails a day, 3,000 a month, one domain, September 2026):

1. In `cloudflare.config.ts`, in `worker.env`, uncomment `MAIL_ADAPTER: bindings.text("resend"),` and set `MAIL_FROM` to an address on a domain verified in Resend, for example `MAIL_FROM: bindings.text("Live Q&A <noreply@yourdomain.com>"),`.
2. Store the API key as a secret: put `RESEND_API_KEY=<key>` in `.prod.vars` (git-ignored), then run `ocre secrets push RESEND_API_KEY --file .prod.vars`. A Worker must exist before it can have secrets, so run this after the first `ocre deploy` if the Worker is new.

See [Email](../guides/email.md) for the `cloudflare` adapter and [Configuration](../reference/configuration.md#mail_adapter) for every variable.

### ocre deploy

```sh
ocre deploy
```

In order, `ocre deploy`:

1. Checks the locale files (when the app has translations) and the `wasm32-unknown-unknown` target, as `ocre dev` does.
2. Looks up the D1 database named in `cloudflare.config.ts` (`qa`) with `cf d1 list` and creates it the first time.
3. Creates the other Cloudflare resources `cloudflare.config.ts` names that are missing: queues, R2 buckets, KV namespaces without an `id`. This app uses none of them; the Durable Object namespace of the realtime channels is created by the deploy itself, from the `OcreChannel` export.
4. Asks `cf workers secrets list` whether the Worker has `SECRET_KEY_BASE`. A new Worker has none, so a fresh random one is uploaded with the deploy and saved in `.prod.vars`. An existing secret is never replaced, since that would sign every user out.
5. Applies the pending migrations to the production database (`cf d1 migrations apply`), before the new code goes live.
6. Deploys with `cf deploy --secrets-file`, which builds the Worker in release mode (optimized for size, slower to compile than `ocre dev`) and uploads it. The secrets file (`.wrangler/ocre-secrets.json`, deleted afterwards) holds the new `SECRET_KEY_BASE`, or `{}`: it is passed on every deploy because cf keeps the Worker's other secrets only when one is given.

cf's own output is shown as it runs; the command then ends with:

```text
Created the SECRET_KEY_BASE secret on Cloudflare
Saved it in .prod.vars (git-ignored): back it up, Cloudflare never gives it back

https://qa.<your-subdomain>.workers.dev
```

The first two lines appear only on the deploy that created the secret. With `--json`, the result is `{"command": "deploy", "ok": true, "secret_created": true, "secret_saved": ".prod.vars", "url": "https://qa.<your-subdomain>.workers.dev"}`. Run `ocre deploy` again after each change: every deploy migrates the database first, so the new code never runs against an old schema.

Free-plan limits that matter for this app (September 2026, [Workers limits](https://developers.cloudflare.com/workers/platform/limits/), [Durable Objects pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)): 100,000 Worker requests a day and 10 ms of CPU per request, and 100,000 Durable Object requests a day. Each question, vote or moderation is one Worker request and two Durable Object requests (one per channel); each browser that opens a room adds one of each for its WebSocket, then costs nothing while it waits, since the channel's object hibernates, and messages the server sends are free. A room of 300 people who each vote ten times uses about 3,300 Worker requests (plus page loads) and 6,300 Durable Object requests. [Free-plan limits](../reference/limits.md) lists the rest.

## Next steps

- [Realtime](../guides/realtime.md): channels, broadcasts from jobs, presence, relaying client messages.
- [Models and migrations](../guides/models.md): queries, associations, changing columns with `ocre g migration`.
- [Authentication](../guides/authentication.md): `CurrentUser`, ownership checks, OAuth, JWTs and API keys.
- [Controllers and routing](../guides/controllers.md), [Views, helpers and forms](../guides/views.md) and [htmx](../guides/htmx.md): handlers, templates, partial updates with htmx.
- [Background jobs and schedules](../guides/jobs.md): send a summary to the host after the event, clean up old events every night.
- [Deployment](../guides/deployment.md): environments, secrets, custom domains.
- [CLI commands](../reference/cli.md) and [Generators](../reference/generators.md): every command and flag.
