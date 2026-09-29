# Controllers, routing, views and htmx

Controllers in Ocre are plain axum handlers grouped in one module per resource, each with a `routes()` function merged into the router in `src/lib.rs`; views are askama templates compiled into the Worker, and htmx adds interactivity by swapping HTML fragments. This page walks through the generated scaffold controller, then shows how to write your own pages and htmx endpoints.

## Before you start

- A full-stack Ocre app from `ocre new` (not `--api`: API-only apps have no templates; see [JSON APIs](json-apis.md)). The examples use the blog starter, `ocre new blog --starter blog`, whose `src/posts.rs` is the scaffold of `Post title:string body:text published:boolean`.
- `ocre dev` running to try the pages (it serves `http://localhost:8787` unless you pass `--port`).
- Free plan (September 2026, [Workers limits](https://developers.cloudflare.com/workers/platform/limits/)): 100,000 requests a day and 10 ms of CPU per request. Awaiting D1 does not count as CPU; rendering a template is plain string building, a fraction of a millisecond for a normal page.

## Generate a scaffold

```sh
ocre g scaffold Comment author:string body:text post:references
```

```text
  create  migrations/0003_create_comments.sql
  create  src/models/comment.rs
  create  src/comments.rs
  create  templates/comments/index.html
  create  templates/comments/show.html
  create  templates/comments/new.html
  create  templates/comments/edit.html
  create  templates/comments/_form.html
  update  src/models/post.rs
  update  src/models/mod.rs
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/comments
```

The model is explained in [Models and migrations](models.md); this page covers `src/comments.rs` (the controller) and `templates/comments/` (the views). In an API-only app, `ocre g scaffold` generates a JSON API instead.

## Routes

`routes()` at the top of the controller lists every route of the resource:

```rust
pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/posts", get(index).post(create))
        .route("/posts/new", get(new))
        .route("/posts/{id}", get(show).post(update))
        .route("/posts/{id}/edit", get(edit))
        .route("/posts/{id}/delete", post(delete))
}
```

| Method | Path | Handler | Does |
|---|---|---|---|
| GET | `/posts` | `index` | List, newest first (`?limit=&offset=`, 50 by default) |
| GET | `/posts/new` | `new` | Empty form |
| POST | `/posts` | `create` | Create, then redirect to `/posts/{id}` with a flash; 422 and the form again when invalid |
| GET | `/posts/{id}` | `show` | One record, or 404 |
| GET | `/posts/{id}/edit` | `edit` | Form filled with the record |
| POST | `/posts/{id}` | `update` | Update, then redirect; 422 and the form again when invalid |
| POST | `/posts/{id}/delete` | `delete` | Delete, then redirect to `/posts` |

Update and delete use `POST` because HTML forms can only send `GET` and `POST`. Rails simulates `PATCH`/`DELETE` with a hidden `_method` field; Ocre keeps the routes the browser actually sends, so the pages work without JavaScript and without method overriding. JSON APIs use real `PATCH` and `DELETE` (see [JSON APIs](json-apis.md#routes)).

Paths use axum 0.8 syntax: `{id}` is a parameter, `{*rest}` a wildcard. A request for a path no route matches gets an empty 404 response.

### List the routes

`ocre routes` reads `src/lib.rs` and the modules it merges, without building the app, and prints every route; an argument filters by method, path or handler (case-insensitive substring):

```sh
ocre routes comments
```

```text
METHOD  PATH                   HANDLER
GET     /comments              comments::index
POST    /comments              comments::create
GET     /comments/new          comments::new
GET     /comments/{id}         comments::show
POST    /comments/{id}         comments::update
POST    /comments/{id}/delete  comments::delete
GET     /comments/{id}/edit    comments::edit
```

It finds `.route("<path>", ...)` calls, follows `.merge(<module>::routes())` into `src/<module>.rs` (or `src/<module>/mod.rs`) and knows that `ocre::graphql::routes(..)` serves `GET` and `POST /graphql`; other constructions (such as `.nest(..)`) are skipped. `--json` returns the list in `routes`:

```json
{"command":"routes","ok":true,"routes":[{"handler":"posts::index","method":"GET","path":"/posts"},{"handler":"posts::create","method":"POST","path":"/posts"},...]}
```

## Register a module in src/lib.rs

`src/lib.rs` is the Worker's entry point and the app's router. Generators add modules and routes after two marker comments, which must stay in place:

```rust
// ocre:modules
mod comments;
mod posts;
mod models;

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> worker::Result<worker::web_sys::Response> {
    ocre::serve(routes(), req, env).await
}

fn routes() -> Router<Ctx> {
    Router::new()
        .route("/", get(home))
        .route("/up", get(up))
        // ocre:routes
        .merge(comments::routes())
        .merge(posts::routes())
}
```

For a module you write by hand, add `mod <name>;` under `// ocre:modules` and `.merge(<name>::routes())` under `// ocre:routes`. `ocre::serve` wraps the router with sessions, CSRF protection, CORS and security headers (see [Sessions, flash and security](security.md)), and builds the per-request `Ctx`. After `ocre g locale`, `.layer(ocre::i18n::layer(&LOCALES))` must stay last in the chain.

## Handlers and extractors

A handler is an `async fn` whose arguments are axum extractors and whose return type implements `IntoResponse`. Handlers do not need `#[worker::send]`: every Ocre type is `Send`.

| Extractor | From | Gives | On failure |
|---|---|---|---|
| `State(ctx): State<Ctx>` | the router state | `Ctx`: `ctx.db()?`, `ctx.env()` and the bindings | never fails |
| `Path(id): Path<i64>` | `{id}` in the path | the parsed value; a tuple or struct for several | 400 `Invalid URL: Cannot parse ...` |
| `Query(q): Query<T>` | the query string, into a serde struct | `T` | 400 |
| `Form(form): Form<T>` | a URL-encoded form body | `T` | 415 without `Content-Type: application/x-www-form-urlencoded`; 422 when it does not deserialize (e.g. `published=maybe` for a `bool`) |
| `ocre::Json(body): ocre::Json<T>` | a JSON body | `T` | JSON 400 (see [JSON APIs](json-apis.md#errors)) |
| `page: ocre::Page` | `?limit=&offset=` | `page.limit` (1-100, default 50), `page.offset` | JSON 400 `limit must be between 1 and 100` |
| `session: ocre::Session` | the encrypted session cookie | `get`, `insert`, `remove`, `clear`, `flash` | its methods fail with a 500 (logged) when `SECRET_KEY_BASE` is missing |
| `flash: ocre::Flash` | messages set by the previous request (read once, then removed) | `flash.notice()`, `flash.alert()`, `flash.get(kind)` | 500 when `SECRET_KEY_BASE` is missing |
| `Htmx(is_htmx): ocre::Htmx` | the `HX-Request: true` header | `bool` | never fails |
| `ocre::storage::Multipart<LIMIT>` | a `multipart/form-data` body | text fields and files | 413 above `LIMIT` bytes (see [File storage](files.md)) |
| `i18n: ocre::i18n::I18n` | the request's locale | translations (see [Translations](i18n.md)) | |

The extractor that reads the body (`Form`, `Json`, `Multipart`) must be the last argument, an axum rule. After `ocre g auth`, `CurrentUser` and `BearerUser` are extractors too (see [Authentication](authentication.md)).

## Responses

| Return | Response |
|---|---|
| `Result<Html<String>>` from `ocre::render(&view)` | 200 with the rendered template |
| `Redirect::to("/posts/1")` | 303 See Other with `Location` (the browser follows with a `GET`) |
| `(StatusCode::UNPROCESSABLE_ENTITY, render(&view)?)` | the page with another status |
| `Result<Response>` with `.into_response()` on each branch | handlers that answer differently per case |
| `Err(ocre::Error)` | an HTML error page with the error's status |

`ocre::Error` and its statuses:

| Variant | Status | Page shows |
|---|---|---|
| `Error::NotFound` | 404 | `Not found` |
| `Error::BadRequest(msg)`, built with `Error::bad_request("...")` | 400 | the message |
| `Error::Unauthorized` | 401 | `Unauthorized` |
| `Error::Forbidden` | 403 | `Forbidden` |
| `Error::Invalid(fields)` | 422 | `Validation failed` and each field error |
| `Error::PayloadTooLarge(msg)` | 413 | the message |
| `Error::Internal(msg)`, built with `Error::internal("...")` | 500 | `Internal server error`; the message goes to the Worker log only |

`?` converts `worker::Error` and askama errors into `Error::Internal`. For a missing record, `option.or_404()?` (trait `ocre::OptionExt`) turns `None` into `Error::NotFound`:

```sh
curl -s -w '\n%{http_code}\n' http://localhost:8787/posts/999
```

```text
<h1>404</h1><p>Not found</p>
404
```

## The scaffold controller

`src/posts.rs`, as generated by the blog starter, in order.

The form struct holds what the browser submits, as text, so a typo in a number becomes a field error rather than a rejected request:

```rust
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PostForm {
    pub title: String,
    pub body: String,
    /// Unchecked checkboxes are not submitted.
    pub published: bool,
}
```

`from_record` fills it for the edit page; `to_new` and `to_changes` parse it into the model's `NewPost` and `PostChanges` and run the model's `validate()` (see [Validations](validations.md#html-forms-re-render)).

One template struct per page, each bound to a file in `templates/posts/`:

```rust
#[derive(Template)]
#[template(path = "posts/index.html")]
struct IndexView {
    flash: Flash,
    posts: Vec<Post>,
}
```

Reading handlers call the model and render:

```rust
async fn index(State(ctx): State<Ctx>, flash: Flash, page: Page) -> Result<Html<String>> {
    render(&IndexView { flash, posts: post::all(&ctx, page).await? })
}

async fn show(State(ctx): State<Ctx>, flash: Flash, Path(id): Path<i64>) -> Result<Html<String>> {
    render(&ShowView { flash, post: post::find(&ctx, id).await?.or_404()? })
}
```

Writing handlers follow Rails' pattern: on success, set a flash and redirect; on `Error::Invalid`, render the form again with status 422 and the typed values; any other error goes up with its status:

```rust
async fn create(State(ctx): State<Ctx>, session: Session, Form(form): Form<PostForm>) -> Result<Response> {
    let created = match form.to_new() {
        Ok(new) => post::create(&ctx, new).await,
        Err(err) => Err(err),
    };
    match created {
        Ok(record) => {
            session.flash("notice", "Post was successfully created.")?;
            Ok(Redirect::to(&format!("/posts/{}", record.id)).into_response())
        }
        Err(Error::Invalid(errors)) => {
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&NewView { form, errors })?).into_response())
        }
        Err(err) => Err(err),
    }
}
```

```sh
curl -si -X POST http://localhost:8787/posts -d 'title=Third&body=Hello+again'
```

```text
HTTP/1.1 303 See Other
Location: /posts/3
Set-Cookie: _ocre_session=...; HttpOnly; SameSite=Lax; Path=/
...
```

The next `GET /posts/3` with that cookie shows `<p class="notice">Post was successfully created.</p>`, and the flash is gone after it. `delete` returns `Error::NotFound` when the model's `delete` returns `false`.

## Views

Templates are [askama](https://docs.rs/askama) files in `templates/`, compiled into the Worker at build time: a typo in a template is a compile error, and rendering needs no file access. A template struct names its file with `#[template(path = "posts/show.html")]`, and its fields are the template's variables; `ocre::render(&view)` renders it into `Html<String>` (an `Error::Internal` if rendering fails).

Because templates are compiled in, `ocre dev` must rebuild the Worker to show a template change. Wrangler's watcher follows `src/` (its default `watch_dir`), so a change to a file under `templates/` alone is not picked up: save any `.rs` file under `src/` (or restart `ocre dev`) to rebuild.

Pages extend `templates/layout.html`, which defines the `title` and `content` blocks and loads htmx:

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{% block title %}blog{% endblock %}</title>
  <script src="https://unpkg.com/htmx.org@2.0.4" crossorigin="anonymous"></script>
  <style>
    ...
  </style>
</head>
<body>
  <header><a href="/">blog</a></header>
  <main>{% block content %}{% endblock %}</main>
</body>
</html>
```

```html
{% extends "layout.html" %}

{% block title %}Post {{ post.id }}{% endblock %}

{% block content %}
<h1>Post {{ post.id }}</h1>
{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
...
{% endblock %}
```

`new.html` and `edit.html` share the fields through `{% include "posts/_form.html" %}`; askama's syntax (`{% for %}`, `{% if let %}`, filters) is in the [askama book](https://askama.readthedocs.io/).

`{{ value }}` is HTML-escaped: `Tom & Jerry` renders as `Tom &#38; Jerry`, and a `<script>` typed into a form shows as text. Never apply `|safe` to anything a user typed. `.txt` templates (emails) are not escaped.

## A controller of your own

A page listing unpublished posts, with a search box, in a new module. Templates can also be inline (`source = "..."`, `ext = "html"`); files under `templates/` are usual for anything longer:

```rust,check
// src/drafts.rs
use askama::Template;
use axum::{
    Router,
    extract::{Path, Query, State},
    response::Html,
    routing::get,
};
use ocre::{Ctx, Flash, OptionExt, Page, Result, params, render};
use serde::Deserialize;

use crate::models::post::Post;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/drafts", get(index)).route("/drafts/{id}", get(show))
}

/// `?q=` filters by title; `?limit=&offset=` come from `Page`.
#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    q: String,
}

#[derive(Template)]
#[template(
    source = r#"{% extends "layout.html" %}
{% block title %}Drafts{% endblock %}
{% block content %}
<h1>Drafts</h1>
{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
<form method="get"><input name="q" value="{{ q }}"> <button>Search</button></form>
<ul>
{% for post in posts %}<li><a href="/drafts/{{ post.id }}">{{ post.title }}</a></li>{% endfor %}
</ul>
{% endblock %}"#,
    ext = "html"
)]
struct IndexView {
    flash: Flash,
    q: String,
    posts: Vec<Post>,
}

#[derive(Template)]
#[template(
    source = r#"{% extends "layout.html" %}
{% block title %}{{ post.title }}{% endblock %}
{% block content %}<h1>{{ post.title }}</h1><p>{{ post.body }}</p>{% endblock %}"#,
    ext = "html"
)]
struct ShowView {
    post: Post,
}

async fn index(
    State(ctx): State<Ctx>,
    flash: Flash,
    page: Page,
    Query(search): Query<Search>,
) -> Result<Html<String>> {
    let posts = ctx
        .db()?
        .all(
            "SELECT * FROM posts WHERE published = 0 AND title LIKE ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3",
            params![format!("%{}%", search.q), page.limit, page.offset],
        )
        .await?;
    render(&IndexView { flash, q: search.q, posts })
}

async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Html<String>> {
    let post: Post = ctx
        .db()?
        .first("SELECT * FROM posts WHERE id = ?1 AND published = 0", params![id])
        .await?
        .or_404()?;
    render(&ShowView { post })
}
```

Register it (`mod drafts;` and `.merge(drafts::routes())` in `src/lib.rs`), then with the seeded posts "Hello" (published) and "Draft":

```sh
curl -s 'http://localhost:8787/drafts?q=Dra'
```

```html
...
  <main>
<h1>Drafts</h1>

<form method="get"><input name="q" value="Dra"> <button>Search</button></form>
<ul>
<li><a href="/drafts/2">Draft</a></li>
</ul>
</main>
...
```

`GET /drafts/1` (published) answers `<h1>404</h1><p>Not found</p>` with status 404, and `GET /drafts/abc` a 400 with the text ``Invalid URL: Cannot parse `abc` to a `i64` ``. The SQL is here to keep the example in one file; in an app, put it in `src/models/post.rs` as a `drafts(ctx, term, page)` function (see [Models](models.md#custom-queries)).

## htmx: partial responses

htmx sends requests from HTML attributes and swaps the HTML it gets back into the page, so interactive pages need no custom JavaScript. The layout already loads it. Every htmx request carries `HX-Request: true`; the `ocre::Htmx` extractor reads it, so one handler can answer a fragment to htmx and a redirect to a plain form post.

A "Publish" button that updates the post's status in place:

```rust,check
// src/publishing.rs
use askama::Template;
use axum::{
    Router,
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::post,
};
use ocre::{Ctx, Htmx, OptionExt, Result, Session, render};

use crate::models::post::{self, Post, PostChanges};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/posts/{id}/publish", post(publish))
}

/// The fragment htmx swaps into the page: the same element, updated.
#[derive(Template)]
#[template(
    source = r#"<span id="post-{{ post.id }}-status">{% if post.published %}Published{% else %}Draft{% endif %}</span>"#,
    ext = "html"
)]
struct StatusPartial {
    post: Post,
}

async fn publish(
    State(ctx): State<Ctx>,
    Htmx(is_htmx): Htmx,
    session: Session,
    Path(id): Path<i64>,
) -> Result<Response> {
    let changes = PostChanges { published: Some(true), ..Default::default() };
    let post = post::update(&ctx, id, changes).await?.or_404()?;
    if is_htmx {
        // htmx request: answer with the fragment only.
        return Ok(render(&StatusPartial { post })?.into_response());
    }
    // Plain form post (JavaScript disabled): full redirect, as usual.
    session.flash("notice", "Post was published.")?;
    Ok(Redirect::to(&format!("/posts/{}", post.id)).into_response())
}
```

In `templates/posts/show.html`, the status and a form that htmx takes over (without JavaScript, the form still posts and redirects):

```html
<p><span id="post-{{ post.id }}-status">{% if post.published %}Published{% else %}Draft{% endif %}</span></p>
<form action="/posts/{{ post.id }}/publish" method="post"
      hx-post="/posts/{{ post.id }}/publish" hx-target="#post-{{ post.id }}-status" hx-swap="outerHTML">
  <button type="submit">Publish</button>
</form>
```

```sh
curl -s -X POST http://localhost:8787/posts/2/publish -H 'HX-Request: true'
```

```html
<span id="post-2-status">Published</span>
```

Without the header, the same request gets `303 See Other` with `Location: /posts/2`. In a browser, clicking "Publish" replaces `<span id="post-2-status">Draft</span>` with `<span id="post-2-status">Published</span>` without leaving the page. Keep fragments in their own template (a `_status.html` partial file, or an inline template as above) and reuse it from the full page, so both render the same markup.

htmx requests from your own pages are same-origin, so the CSRF check lets them through with no token. A cross-site `POST` is refused:

```sh
curl -s -X POST http://localhost:8787/posts/1/delete -H 'Sec-Fetch-Site: cross-site'
```

```text
Forbidden: cross-site request. Add the origin to ALLOWED_ORIGINS to allow it.
```

(status 403; see [Sessions, flash and security](security.md)). For pages that update live for every visitor, see [Realtime](realtime.md).

## Static files and the health check

Files in `public/` (CSS, images, `robots.txt`, `favicon.ico`) are served by Workers Static Assets before the Worker runs: they cost no Worker request and no CPU, and are not counted in the 100,000 requests a day. `public/robots.txt` is `/robots.txt`. Because the Worker does not run, these responses do not get Ocre's security headers. A file in `public/` wins over a route with the same path.

`GET /up` answers `200 OK` with the body `OK` whenever the Worker runs, like Rails' `/up`; point uptime monitors at it. Keep it free of database queries:

```sh
curl -si http://localhost:8787/up
```

```text
HTTP/1.1 200 OK
Transfer-Encoding: chunked
Content-Type: text/plain; charset=utf-8
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0

OK
```

## See also

- [Models and migrations](models.md): the functions controllers call
- [Validations](validations.md): errors and form re-rendering
- [JSON APIs and GraphQL](json-apis.md): `ocre g api`, `ApiResult`, `Json`
- [Sessions, flash and security](security.md): sessions, flash, CSRF, CORS, headers
- [Authentication](authentication.md): `CurrentUser` and protected pages
- [Realtime](realtime.md): live updates with htmx and WebSockets
- [Generators](../reference/generators.md#ocre-g-scaffold), [CLI commands](../reference/cli.md#ocre-routes)
- [API index](../api-index.md) and the [rustdoc](/api/ocre/index.html)
