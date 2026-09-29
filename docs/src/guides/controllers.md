# Controllers and routing

Controllers in Ocre are plain axum handlers grouped in one module per resource, each with a `routes()` function merged into the router in `src/lib.rs`, and a `paths` module that builds the URLs of its pages. This page walks through the generated scaffold controller, then covers routing (nested resources, namespaces, redirects), what a handler can read from the request and answer, error pages, and middleware. Templates and forms are in [Views, helpers and forms](views.md), interactivity in [htmx](htmx.md), CSS and JavaScript in [Assets](assets.md).

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

The model is explained in [Models and migrations](models.md); this page covers `src/comments.rs` (the controller); the templates are described in [Views](views.md). In an API-only app, `ocre g scaffold` generates a JSON API instead.

## Routes

`routes()` at the top of the controller lists every route of the resource (Rails' `resources :posts`):

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
| GET | `/posts` | `index` | List, newest first (`?limit=&offset=`, 50 by default), with Previous / Next links |
| GET | `/posts/new` | `new` | Empty form |
| POST | `/posts` | `create` | Create, then redirect to `/posts/{id}` with a flash; 422 and the form again when invalid |
| GET | `/posts/{id}` | `show` | One record, or 404 |
| GET | `/posts/{id}/edit` | `edit` | Form filled with the record |
| POST | `/posts/{id}` | `update` | Update, then redirect; 422 and the form again when invalid |
| POST | `/posts/{id}/delete` | `delete` | Delete, then redirect to `/posts` |

Update and delete use `POST` because HTML forms can only send `GET` and `POST`. Rails simulates `PATCH`/`DELETE` with a hidden `_method` field; Ocre keeps the routes the browser actually sends, so the pages work without JavaScript and without method overriding. JSON APIs use real `PATCH` and `DELETE` (see [JSON APIs](json-apis.md#routes)). To drop an action (Rails' `only:`/`except:`), delete its route and handler; to rename a path (`path:`), edit the strings here and in `paths`.

Paths use axum 0.8 syntax: `{id}` is a parameter, `{*rest}` a wildcard. A path no route matches gets the app's 404 page (see [Error pages](#error-pages)).

### Path helpers

Below `routes()`, the `paths` module builds the URL of each page (Rails' `posts_path`, `new_post_path`, `post_path(post)`, `edit_post_path(post)`):

```rust
pub mod paths {
    use std::fmt::Display;

    pub fn index() -> &'static str {
        "/posts"
    }

    pub fn new() -> &'static str {
        "/posts/new"
    }

    pub fn show(id: impl Display) -> String {
        format!("/posts/{id}")
    }

    pub fn edit(id: impl Display) -> String {
        format!("/posts/{id}/edit")
    }

    pub fn delete(id: impl Display) -> String {
        format!("/posts/{id}/delete")
    }
}
```

Handlers redirect with them (`Redirect::to(&paths::show(record.id))`) and templates link with them: askama resolves `paths` next to the template's struct, so the controller's templates write `<a href="{{ paths::edit(post.id) }}">`, and any other module `crate::posts::paths::show(id)`. Changing a URL is then one edit in `routes()` and one in `paths`. Attachment fields add one function per file (`paths::avatar(id)`). They are plain functions, so a custom URL helper (Rails' `direct`, `default_url_options` such as a locale prefix) is a function you add, with the parameters it needs.

### List the routes

`ocre routes` reads `src/lib.rs` and the modules it merges or nests, without building the app, and prints every route; an argument filters by method, path or handler (case-insensitive substring):

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

It finds `.route("<path>", ...)` calls, follows `.merge(<module>::routes())` and `.nest("<prefix>", <module>::routes())` into `src/<module>.rs` (or `src/<module>/mod.rs`), and knows that `ocre::graphql::routes(..)` serves `GET` and `POST /graphql`. Routes whose handler is a closure, and routers built inline or in variables, are skipped. `--json` returns the list in `routes`:

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
        .fallback(not_found)
        .layer(map_response(error_page))
        .layer(content_security_policy())
        .layer(permissions_policy())
}
```

For a module you write by hand, add `mod <name>;` under `// ocre:modules` and `.merge(<name>::routes())` under `// ocre:routes`. `ocre::serve` wraps the router with sessions, CSRF protection, CORS and security headers (see [Sessions, flash and security](security.md)), and builds the per-request `Ctx`. `/` is the root route (Rails' `root`), `/up` the health check. `.fallback` and the error-page layer come after the routes (see [Error pages](#error-pages)), followed by the Content-Security-Policy and Permissions-Policy layers (see [Security](security.md)); after `ocre g locale`, `.layer(ocre::i18n::layer(&LOCALES))` must stay the very last call.

## Routing recipes

axum's `Router` covers Rails' routing DSL with ordinary method calls. One module showing the common shapes:

```rust,check
// src/library.rs
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use ocre::{Ctx, OptionExt, Result, Session};

use crate::models::{comment, post};

pub fn routes() -> Router<Ctx> {
    Router::new()
        // Nested resource (Rails' `resources :posts do resources :comments end`).
        .route("/posts/{post_id}/comments/{id}", get(post_comment))
        // Singular resource (Rails' `resource :profile`): no id, the session says whose.
        .route("/profile", get(profile))
        // Member and collection routes: more paths under the resource.
        .route("/posts/{id}/preview", get(preview))
        .route("/posts/drafts", get(drafts))
        // Redirects (Rails' `get "/articles/:id", to: redirect("/posts/%{id}")`).
        .route("/articles", get(|| async { Redirect::permanent("/posts") }))
        .route("/articles/{id}", get(|Path(id): Path<i64>| async move { Redirect::permanent(&format!("/posts/{id}")) }))
        // Glob (Rails' `get "pages/*path"`): `path` is the rest, e.g. `help/billing`.
        .route("/pages/{*path}", get(page))
        // Namespace (Rails' `namespace :admin`): every admin route under /admin.
        .nest("/admin", admin_routes())
}

fn admin_routes() -> Router<Ctx> {
    Router::new().route("/", get(|| async { "Admin home" })).route("/posts/{id}", get(preview))
}

/// Both ids come from the path, in order; a comment of another post is a 404.
async fn post_comment(State(ctx): State<Ctx>, Path((post_id, id)): Path<(i64, i64)>) -> Result<String> {
    let comment = comment::find(&ctx, id).await?.filter(|c| c.post_id == post_id).or_404()?;
    Ok(comment.body)
}

async fn profile(session: Session) -> Result<Response> {
    match session.get::<i64>("user_id")? {
        Some(id) => Ok(format!("user {id}").into_response()),
        None => Ok(Redirect::to("/login").into_response()),
    }
}

async fn preview(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<String> {
    Ok(post::find(&ctx, id).await?.or_404()?.title)
}

async fn drafts() -> &'static str {
    "drafts"
}

async fn page(Path(path): Path<String>) -> Response {
    match path.as_str() {
        "help/billing" => "Billing help".into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
```

`/posts/drafts` and `/posts/{id}` can coexist: axum prefers the static segment. A typed parameter is its constraint (Rails' `constraints: { id: /\d+/ }`): `Path<i64>` answers `400 Bad Request` to `/posts/abc`, with the app's error page. Everything else:

| Rails | Ocre (axum) |
|---|---|
| `resources :posts` (seven actions, helpers) | `ocre g scaffold`: the routes, handlers and `paths` |
| `resources :photos, :books` | one module per resource, each merged |
| `get "x", to: "c#a"`, `match via:` | `.route("/x", get(a).post(b))`; unmatched methods answer 405 |
| Optional segments `(/:page)` | two routes to the same handler, or a query parameter |
| `defaults: { format: "json" }` | `#[serde(default)]` on the `Query` struct |
| `as:` named routes, `direct`, `resolve` | functions in `paths` |
| `scope "/:account_id"` | `.nest("/{account_id}", accounts::routes())`; handlers read `Path` |
| `scope module:` / `path:` | the Rust module is independent of the prefix you nest at |
| `concern :commentable` | a function returning a `Router<Ctx>`, merged or nested in each resource |
| `shallow: true` | declare the member routes without the parent prefix |
| Subdomain / request constraints | check `Host` or headers in a middleware or the handler (below) |
| `.:format` segments | the `Accept` header with `ocre::Format` (below), or a `.json` route of its own |
| `mount` a Rack app | `.merge(other_router)` or `.nest_service("/x", service)`; `ocre::graphql::routes` is one |
| `draw` split route files | one `routes()` per module |
| Unicode paths | write them percent-encoded (`.route("/caf%C3%A9", ..)`): axum matches the raw path, which browsers send encoded, so a `"/café"` route never matches; `Path` parameters arrive decoded (`café`) |
| Translated path segments | one route per language, pointing to the same handler (see [Translations](i18n.md)) |
| `rails routes` | `ocre routes` |

## Handlers and extractors

A handler is an `async fn` whose arguments are axum extractors and whose return type implements `IntoResponse`. Handlers do not need `#[worker::send]`: every Ocre type is `Send`. Params are typed per source instead of Rails' merged `params` hash, and a form or JSON struct lists exactly the fields it accepts, which replaces strong parameters.

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
| `format: ocre::Format` | the `Accept` header | `Format::Html`, `Json`, `Xml`, `Text` or `Other` | never fails |
| `Htmx(is_htmx): ocre::Htmx` | the `HX-Request: true` header | `bool` | never fails |
| `RemoteIp(ip): ocre::RemoteIp` | Cloudflare's `CF-Connecting-IP` | `Option<IpAddr>` | never fails |
| `RequestId(id): ocre::RequestId` | Cloudflare's `CF-Ray`, else `X-Request-Id`, else random | `String` | never fails |
| `headers: HeaderMap` | all request headers | `headers.get("user-agent")`, cookies... | never fails |
| `OriginalUri(uri)`, `method: Method` | the request line | the full URI (path and query), the method | never fails |
| `ocre::storage::Multipart<LIMIT>` | a `multipart/form-data` body | text fields and files | 413 above `LIMIT` bytes (see [File storage](files.md)) |
| `i18n: ocre::i18n::I18n` | the request's locale | translations (see [Translations](i18n.md)) | |

The extractor that reads the body (`Form`, `Json`, `Multipart`) must be the last argument, an axum rule. axum's `Form`, `Json` and body extractors refuse bodies over 2 MB with `413` (Loco's `limit_payload`); a route that needs more adds `.layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024))`. Workers accept request bodies up to 100 MB. After `ocre g auth`, `CurrentUser` and `BearerUser` are extractors too (see [Authentication](authentication.md)); an extractor is also how Ocre does Rails' `before_action`: a handler that takes `CurrentUser` only runs for signed-in users.

Cookies other than the session are headers (Rails' `cookies`): read them from `HeaderMap`, and set one by returning a header, `([(header::SET_COOKIE, "theme=dark; Path=/; Max-Age=31536000; HttpOnly; Secure; SameSite=Lax")], body)`; a cookie is deleted with `Max-Age=0`. Values that must not be forged or read belong in the `Session`, which is signed and encrypted.

## Responses

| Return | Response |
|---|---|
| `Result<Html<String>>` from `ocre::render(&view)` | 200 with the rendered template |
| `Redirect::to("/posts/1")` | 303 See Other with `Location` (the browser follows with a `GET`) |
| `Redirect::permanent(..)`, `Redirect::temporary(..)` | 308, 307 |
| `ocre::redirect_back(&headers, "/")` | 303 to the referring page of this app, else to the fallback (Rails' `redirect_back_or_to`) |
| `ocre::HxRedirect(path)` | 200 with `HX-Redirect`: htmx loads the page in full (see [htmx](htmx.md#redirecting-from-an-htmx-request)) |
| `(StatusCode::UNPROCESSABLE_ENTITY, render(&view)?)` | the page with another status |
| `StatusCode::NO_CONTENT` | a status without a body (Rails' `head`) |
| `([(header::CACHE_CONTROL, "no-store")], body)` | extra headers before the body |
| `"text"` / `String`, `Html(..)` | `text/plain`, `text/html` |
| `ocre::storage::send_data(..)`, `ocre::storage::serve(..)` | a file download (below) |
| `Result<Response>` with `.into_response()` on each branch | handlers that answer differently per case |
| `Err(ocre::Error)` | the error page with the error's status |

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

`?` converts `worker::Error` and askama errors into `Error::Internal`. For a missing record, `option.or_404()?` (trait `ocre::OptionExt`) turns `None` into `Error::NotFound`. Mapping errors to responses (Rails' `rescue_from`) is a `match` on the `Error` in the handler, as the scaffold does for `Error::Invalid`, or a `map_response` layer for the whole app, as for error pages.

### Formats (respond_to)

`ocre::Format` reads the `Accept` header, so one action can serve a page to browsers and JSON to API clients (Rails' `respond_to`, Loco's `RespondTo`):

```rust,check
use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ocre::{Ctx, Format, Json, OptionExt, Result, render};

use crate::models::post::{self, Post};

#[derive(Template)]
#[template(source = "<h1>{{ post.title }}</h1><p>{{ post.body }}</p>", ext = "html")]
struct PostPage {
    post: Post,
}

/// GET /posts/{id}: HTML for browsers, JSON for `Accept: application/json`.
async fn show(State(ctx): State<Ctx>, format: Format, Path(id): Path<i64>) -> Result<Response> {
    let post = post::find(&ctx, id).await?.or_404()?;
    Ok(match format {
        Format::Html => render(&PostPage { post })?.into_response(),
        Format::Json => Json(post).into_response(),
        _ => StatusCode::NOT_ACCEPTABLE.into_response(),
    })
}
```

```sh
curl -s http://localhost:8787/posts/1 -H 'Accept: application/json'
```

```json
{"id":1,"title":"Hello","body":"First","published":true,"created_at":"2026-09-29 10:00:00","updated_at":"2026-09-29 10:00:00"}
```

A missing `Accept`, `*/*` and `text/html` are `Html`; browsers get HTML. Other representations (Rails' request variants, a phone layout) are another `match` arm or another view struct chosen from a header.

### Files and downloads

`ocre::storage::send_data` sends bytes the handler built as a file (Rails' `send_data`), with the right `Content-Type`, `Content-Length` and a sanitized `Content-Disposition` file name:

```rust,check
use axum::{extract::State, response::Response};
use ocre::{Ctx, Page, Result, storage::{Disposition, send_data}};

use crate::models::post;

/// GET /posts.csv: the latest 100 posts as a spreadsheet.
async fn export(State(ctx): State<Ctx>) -> Result<Response> {
    let mut csv = String::from("id,title,published\n");
    for post in post::all(&ctx, Page::new(100, 0)?).await? {
        csv.push_str(&format!("{},\"{}\",{}\n", post.id, post.title.replace('"', "\"\""), post.published));
    }
    Ok(send_data(csv, "posts.csv", "text/csv", Disposition::Download))
}
```

Files stored in R2 go out with `ocre::storage::serve` (Rails' `send_file` and `Rack::Sendfile`): the Worker hands R2's stream to Cloudflare without copying it through WebAssembly, and answers `Range` and `If-None-Match` requests (see [File storage](files.md)). Generated data should stay small: the body is built in memory (128 MB per Worker) and within the 10 ms of CPU.

Streaming a response as it is produced (Rails' `ActionController::Live`, Server-Sent Events) is not provided: a Worker holding a connection open counts wall time but also needs CPU per chunk. For pages that update live, use WebSockets on a Durable Object (see [Realtime](realtime.md)).

## Error pages

`ocre new` generates `templates/error.html` and two functions in `src/lib.rs` (Rails' `public/404.html`, `422.html` and `500.html`, rendered with the app's layout):

```rust
/// Paths no route matches: the 404 error page.
async fn not_found() -> Error {
    Error::NotFound
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorView<'a> {
    error: &'a ErrorPage,
}

/// Renders error responses (404, 422, 500...) with templates/error.html.
async fn error_page(response: Response) -> Response {
    ocre::error_page(response, |error| render(&ErrorView { error }))
}
```

`routes()` ends with `.fallback(not_found)` and `.layer(map_response(error_page))`. `ocre::error_page` renders the template for every `ocre::Error` a handler returns, for the fallback's 404, and for axum's plain-text rejections (a path that does not parse, a form that does not deserialize), whose message is the status name (`Bad Request`). It keeps the status and headers, and leaves pages that set their own status (a form answered with 422) and JSON errors alone. The template gets an `ocre::ErrorPage`: `error.status` (`{{ error.status.as_u16() }}`), `error.message` and, for a 422, `error.fields`:

```html
{% extends "layout.html" %}

{% block title %}{{ error.message }}{% endblock %}

{% block content %}
<h1>{{ error.message }}</h1>
{% if !error.fields.is_empty() %}
<ul class="errors">
  {% for field in error.fields %}<li>{{ field.full_message() }}</li>{% endfor %}
</ul>
{% endif %}
<p>Error {{ error.status.as_u16() }}. <a href="/">Back to the home page</a></p>
{% endblock %}
```

```sh
curl -s -w '\n%{http_code}\n' http://localhost:8787/posts/999
```

```text
...<h1>Not found</h1>...
404
```

A 500 shows `Internal server error`; the cause goes to the Worker logs only. If the template itself fails, the plain page (`<h1>500</h1><p>Internal server error</p>`) is sent and the failure logged. Requests refused by Ocre's own middleware (a cross-site form post, an unknown host) keep a short text answer. A Rust panic aborts the Worker instance, which Cloudflare answers with its own 500 page: return `Err` instead of `unwrap()` in handlers. API-only apps have a JSON fallback: `{"error": {"status": 404, "message": "Not found"}}`.

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
    page: Page,
    posts: Vec<Post>,
}
```

Reading handlers call the model and render:

```rust
async fn index(State(ctx): State<Ctx>, flash: Flash, page: Page) -> Result<Html<String>> {
    render(&IndexView { flash, page, posts: post::all(&ctx, page).await? })
}

async fn show(State(ctx): State<Ctx>, flash: Flash, Path(id): Path<i64>) -> Result<Html<String>> {
    render(&ShowView { flash, post: post::find(&ctx, id).await?.or_404()? })
}
```

The index page links to the neighbouring pages with `Page::previous` and `Page::next`, which guesses from a full page that more rows follow (no `COUNT(*)`, which would read every row):

```html
<nav class="pagination">
  {% if let Some(previous) = page.previous() %}<a href="?{{ previous.query() }}" rel="prev">Previous</a>{% endif %}
  {% if let Some(next) = page.next(posts.len()) %}<a href="?{{ next.query() }}" rel="next">Next</a>{% endif %}
</nav>
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
            Ok(Redirect::to(&paths::show(record.id)).into_response())
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

`GET /drafts/1` (published) answers the 404 page, and `GET /drafts/abc` the 400 page (`Bad Request`). The SQL is here to keep the example in one file; in an app, put it in `src/models/post.rs` as a `drafts(ctx, term, page)` function (see [Models](models.md#custom-queries)).

## Middleware

Middleware wraps handlers to run code before and after them. `ocre::serve` always runs, outermost first: security headers, host authorization, CORS, cross-origin request protection, and the session cookie (see [Sessions, flash and security](security.md)). The app adds its own with `.layer(..)` on the whole router or `.route_layer(..)` on the routes declared so far; the last layer added runs first. axum's `middleware::from_fn` turns an `async fn` into a layer; its arguments are extractors followed by the request and `next`. `ocre::serve` puts the `Ctx` in the request's extensions, so a middleware reaches the bindings with `Extension<Ctx>`:

```rust,check
use axum::{
    Extension, Router,
    extract::Request,
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use ocre::{Ctx, RequestId};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/admin/stats", get(|| async { "stats" }))
        // Only the routes above: an admin check (Rails' `before_action` on a controller).
        .route_layer(middleware::from_fn(admin_only))
        .route("/status", get(|| async { "OK" }))
        // Every route of this router.
        .layer(middleware::from_fn(request_id_header))
        .layer(middleware::from_fn(maintenance))
}

async fn admin_only(req: Request, next: Next) -> Response {
    let is_admin = req.headers().get("x-admin-token").is_some_and(|token| token == "let-me-in");
    if !is_admin {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

/// Echoes the request id so a user can quote it in a bug report (Loco's `request_id`).
async fn request_id_header(RequestId(id): RequestId, req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

/// Maintenance mode: `MAINTENANCE: bindings.text("on")` in cloudflare.config.ts (or
/// in the dashboard) answers 503 everywhere but /up, without a new build.
async fn maintenance(Extension(ctx): Extension<Ctx>, req: Request, next: Next) -> Response {
    let on = ctx.env().var("MAINTENANCE").is_ok_and(|value| value.to_string() == "on");
    if on && req.uri().path() != "/up" {
        let headers = [("retry-after", "600")];
        return (StatusCode::SERVICE_UNAVAILABLE, headers, "Down for maintenance, back soon.").into_response();
    }
    next.run(req).await
}
```

Rate limiting per action uses the Workers Rate Limiting binding through `ocre::security::rate_limit` (see [Security](security.md)). Loco's and Rails' other middleware map to the platform:

| Middleware | On Ocre |
|---|---|
| Request logging (Loco's `logger`, Rails' request log) | Workers Logs records every request (method, URL, status, CPU time, `console` lines): `observability: { enabled: true }`, in the generated `cloudflare.config.ts`. Free plan: 200,000 events a day, kept 3 days |
| `request_id` | `ocre::RequestId`: Cloudflare's `CF-Ray`, shown next to each request in Workers Logs |
| `remote_ip` | `ocre::RemoteIp`, from `CF-Connecting-IP`, which Cloudflare sets and clients cannot forge |
| `compression` | Cloudflare compresses responses at the edge (Brotli, gzip) |
| `etag` / conditional GET | `ocre::cache::Conditional` and `ETag` (see [Caching](caching.md)) |
| `limit_payload` | axum's 2 MB `DefaultBodyLimit`, per route with `.layer(DefaultBodyLimit::max(n))` |
| `cors` | the `ALLOWED_ORIGINS` variable, or tower-http's `CorsLayer` on a router |
| `secure_headers` | set by `ocre::serve`; a handler's own value wins |
| `catch_panic` | not possible: a panic aborts the WebAssembly instance; return `Err` |
| `timeout_request` | Cloudflare ends requests over the CPU limit (10 ms on the free plan); subrequests have their own timeouts |
| `fallback` | `.fallback(not_found)` in `src/lib.rs` |
| `powered_by` / `X-Powered-By` | not sent; add it with a layer if you want it |
| Server timing, `benchmark` | the Workers clock only moves on I/O, so in-Worker timings are meaningless; Workers Logs reports CPU and wall time per request |
| Config-driven stack, `insert_before`, `delete` | the `.layer(..)` calls in `routes()`, in code |

## Static files and the health check

Files in `public/` (CSS, images, `robots.txt`, `favicon.ico`) are served by Workers Static Assets before the Worker runs: they cost no Worker request and no CPU. See [Assets](assets.md) for caching, CSS and JavaScript tooling.

`GET /up` answers `200 OK` with the body `OK` whenever the Worker runs, like Rails' `/up`; point uptime monitors at it. Keep it free of database queries:

```sh
curl -si http://localhost:8787/up
```

```text
HTTP/1.1 200 OK
Transfer-Encoding: chunked
Content-Type: text/plain; charset=utf-8
content-security-policy: default-src 'self'; script-src 'self' https://unpkg.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'self'
permissions-policy: camera=(), microphone=(), geolocation=(), payment=(), usb=()
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0

OK
```

## See also

- [Views, helpers and forms](views.md): templates, layouts, partials, view helpers and forms
- [htmx](htmx.md): boosted navigation, partial responses, inline editing
- [Assets](assets.md): CSS, JavaScript and images
- [Models and migrations](models.md): the functions controllers call
- [Validations](validations.md): errors and form re-rendering
- [JSON APIs and GraphQL](json-apis.md): `ocre g api`, `ApiResult`, `Json`
- [Sessions, flash and security](security.md): sessions, flash, CSRF, CORS, headers
- [Authentication](authentication.md): `CurrentUser` and protected pages
- [Realtime](realtime.md): live updates with htmx and WebSockets
- [Generators](../reference/generators.md#ocre-g-scaffold), [CLI commands](../reference/cli.md#ocre-routes)
- [API index](../api-index.md) and the [rustdoc](/api/ocre/index.html)
