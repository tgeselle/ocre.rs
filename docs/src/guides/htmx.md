# htmx

Ocre pages get their interactivity from [htmx](https://htmx.org): HTML attributes send requests, and the HTML fragments handlers answer are swapped into the page. There is no JavaScript to write or build for the common cases, every page still works without JavaScript, and handlers stay ordinary axum handlers rendering askama templates. This page covers boosted navigation (Rails' Turbo Drive), partial responses (Turbo Frames), inline editing, validation as you type, live search, infinite scroll and redirects from htmx requests.

## Before you start

- A full-stack app from `ocre new`; its `templates/layout.html` loads htmx 2 from unpkg and turns on boosted navigation. The examples use the blog starter (`ocre new blog --starter blog`).
- Every htmx request is a normal Worker request: it counts toward the free plan's 100,000 requests a day and 10 ms of CPU each. Swapping a fragment instead of a page saves bytes and rendering time, not requests: trigger requests on user actions (`click`, `change`, `revealed`, `delay:300ms`), not on timers.

## Boosted navigation

The generated layout puts `hx-boost="true"` on `<body>`: links and forms load the next page with an AJAX request and swap its `<body>` without a full reload, then update the URL and history (Rails' Turbo Drive). Redirects are followed, so the scaffold's "create, then redirect to the record" works unchanged. Without JavaScript, the same links and forms work as usual.

```html
<meta name="htmx-config" content='{"responseHandling": [{"code": "204", "swap": false}, {"code": "...", "swap": true}]}'>
...
<body hx-boost="true">
```

The `htmx-config` line makes htmx swap error responses too (by default it ignores 4xx and 5xx bodies), so a form answered with `422` and its errors, or the error page of a 404, shows up like any page. Opt a link or form out with `hx-boost="false"`: the scaffold does it for file links, so the browser downloads or displays the file itself.

`hx-confirm` asks before sending (Rails' `data-turbo-confirm`); the scaffold's delete button uses it:

```html
<form action="{{ paths::delete(post.id) }}" method="post" hx-boost="true" hx-confirm="Delete this post?">
  <button type="submit">Delete post</button>
</form>
```

## Partial responses

Every htmx request carries `HX-Request: true`; the `ocre::Htmx` extractor reads it, so one handler can answer a fragment to htmx and a redirect to a plain form post (Rails' Turbo Frames). A "Publish" button that updates the post's status in place:

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

In `templates/posts/show.html`, the status and a form that htmx takes over:

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

Without the header, the same request gets `303 See Other` with `Location: /posts/2`. Keep fragments in their own template (a `_status.html` file, or an inline template as above) and reuse them from the full page, so both render the same markup. htmx requests from your own pages are same-origin, so Ocre's CSRF check lets them through with no token (see [Security](security.md)).

## Inline editing

Click a title to edit it in place; save or cancel swaps the title back. The same `TitleForm` fragment comes back with status 422 and the errors when the title is invalid:

```rust,check
// src/titles.rs
use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use ocre::{Ctx, Error, FieldError, OptionExt, Result, render};
use serde::Deserialize;

use crate::models::post::{self, PostChanges};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/posts/{id}/title", get(show).post(update)).route("/posts/{id}/title/edit", get(edit))
}

#[derive(Template)]
#[template(
    source = r#"<h1 id="title" hx-get="/posts/{{ id }}/title/edit" hx-swap="outerHTML" title="Click to edit">{{ title }}</h1>"#,
    ext = "html"
)]
struct TitleView {
    id: i64,
    title: String,
}

#[derive(Template)]
#[template(
    source = r##"<form id="title" hx-post="/posts/{{ id }}/title" hx-swap="outerHTML">
  <input name="title" value="{{ title }}" autofocus>
  {% for error in errors %}<small class="errors">{{ error.message }}</small>{% endfor %}
  <button type="submit">Save</button>
  <button type="button" hx-get="/posts/{{ id }}/title" hx-target="#title" hx-swap="outerHTML">Cancel</button>
</form>"##,
    ext = "html"
)]
struct TitleForm {
    id: i64,
    title: String,
    errors: Vec<FieldError>,
}

#[derive(Deserialize)]
struct TitleParams {
    title: String,
}

async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Response> {
    let post = post::find(&ctx, id).await?.or_404()?;
    Ok(render(&TitleView { id, title: post.title })?.into_response())
}

async fn edit(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Response> {
    let post = post::find(&ctx, id).await?.or_404()?;
    Ok(render(&TitleForm { id, title: post.title, errors: vec![] })?.into_response())
}

async fn update(State(ctx): State<Ctx>, Path(id): Path<i64>, Form(params): Form<TitleParams>) -> Result<Response> {
    let changes = PostChanges { title: Some(params.title.clone()), ..Default::default() };
    let checked = changes.validate().finish();
    let updated = match checked {
        Ok(()) => post::update(&ctx, id, changes).await,
        Err(err) => Err(err),
    };
    match updated {
        Ok(post) => Ok(render(&TitleView { id, title: post.or_404()?.title })?.into_response()),
        Err(Error::Invalid(errors)) => {
            let form = TitleForm { id, title: params.title, errors };
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&form)?).into_response())
        }
        Err(err) => Err(err),
    }
}
```

The show page includes the title fragment: `<h1 id="title" hx-get="/posts/{{ post.id }}/title/edit" hx-swap="outerHTML">{{ post.title }}</h1>`.

## Validation as you type

A field checks itself when it loses focus: htmx posts the whole form to a validation endpoint (`hx-include="closest form"`) and swaps the field's message. The endpoint runs the model's own rules, so the messages are the ones the form shows on submit:

```rust,check
use askama::Template;
use axum::{Form, extract::Path, response::Html};
use ocre::{FieldError, Result, render};
use serde::Deserialize;

use crate::models::post::NewPost;

#[derive(Default, Deserialize)]
#[serde(default)]
struct Draft {
    title: String,
    body: String,
    published: bool,
}

#[derive(Template)]
#[template(source = "{% for error in errors %}{{ error.message }} {% endfor %}", ext = "html")]
struct FieldMessages {
    errors: Vec<FieldError>,
}

/// POST /posts/validate/{field}: the messages of one field, empty when it is valid.
async fn validate(Path(field): Path<String>, Form(draft): Form<Draft>) -> Result<Html<String>> {
    let new = NewPost { title: draft.title, body: draft.body, published: draft.published };
    let errors = new.validate().errors().iter().filter(|error| error.field == field).cloned().collect();
    render(&FieldMessages { errors })
}
```

```html
<label>Title
  <input name="title" value="{{ form.title }}"
         hx-post="/posts/validate/title" hx-include="closest form" hx-trigger="blur changed"
         hx-target="next .field-error">
</label>
<small class="field-error errors"></small>
```

Register it with `.route("/posts/validate/{field}", post(validate))`. Checks that need the database (uniqueness) stay in `create`, where the form shows them after submit; a validation endpoint that queries D1 costs rows read on every blur.

## Live search

`hx-trigger="input changed delay:300ms"` waits for a pause in typing, so a search costs one request per pause, not per keystroke:

```html
<input type="search" name="q" placeholder="Search posts"
       hx-get="/posts/search" hx-trigger="input changed delay:300ms, search" hx-target="#results">
<ul id="results"></ul>
```

The handler returns the `<li>` rows only. Escape the user's text in `LIKE` patterns with `ocre::escape_like` (see [Models](models.md)).

## Infinite scroll

The last item of a page asks for the next page when it scrolls into view (`hx-trigger="revealed"`), and the answer is inserted after it. The same handler serves the full page to a first visit and the items only to htmx:

```rust,check
use askama::Template;
use axum::{extract::State, response::Html};
use ocre::{Ctx, Htmx, Page, Result, render};

use crate::models::post::{self, Post};

#[derive(Template)]
#[template(
    source = r#"{% for post in posts %}
<li{% if loop.last %}{% if let Some(next) = page.next(posts.len()) %} hx-get="/feed?{{ next.query() }}" hx-trigger="revealed" hx-swap="afterend"{% endif %}{% endif %}>{{ post.title }}</li>
{% endfor %}"#,
    ext = "html"
)]
struct Items {
    page: Page,
    posts: Vec<Post>,
}

#[derive(Template)]
#[template(source = r#"{% extends "layout.html" %}{% block content %}<ul>{{ items|safe }}</ul>{% endblock %}"#, ext = "html")]
struct FeedView {
    items: String,
}

/// GET /feed: 50 posts, then 50 more each time the last one scrolls into view.
async fn feed(State(ctx): State<Ctx>, Htmx(is_htmx): Htmx, page: Page) -> Result<Html<String>> {
    let posts = post::all(&ctx, page).await?;
    let Html(items) = render(&Items { page, posts })?;
    if is_htmx {
        return Ok(Html(items));
    }
    render(&FeedView { items })
}
```

`items|safe` is fine here: `items` is HTML askama rendered and escaped. `Page::next` guesses from a full page that more rows follow, without a `COUNT(*)` query (D1 bills every row a count reads). A classic "Previous / Next" navigation uses the same `Page` methods; the scaffold's index pages have one.

## Redirecting from an htmx request

htmx follows a `303` inside the request and swaps the result into the target, which is right for boosted navigation but wrong when an inline widget should end on another page. `ocre::HxRedirect` answers `200` with the `HX-Redirect` header, and htmx performs a full navigation:

```rust,check
use axum::response::{IntoResponse, Redirect, Response};
use ocre::{Htmx, HxRedirect};

/// After an inline "Create" in a modal, go to the new record's page.
async fn created(Htmx(is_htmx): Htmx) -> Response {
    let target = "/posts/7".to_owned();
    if is_htmx { HxRedirect(target).into_response() } else { Redirect::to(&target).into_response() }
}
```

## Updating several parts of a page

A response can update elements outside its target with out-of-band swaps (Rails' Turbo Streams over HTTP): any element with `hx-swap-oob="true"` and an `id` replaces the element with the same id. `ocre::realtime` sends the same kind of fragments over WebSockets to every open page (see [Realtime](realtime.md)).

```html
<li id="post_7">Updated title</li>
<span id="post-count" hx-swap-oob="true">12 posts</span>
```

## JavaScript beyond htmx

For behaviour htmx does not cover (a date picker, a chart), put the script in `public/` and load it with `<script src="/app.js" defer>` (see [Assets](assets.md)). Prefer script files to inline `<script>` blocks and `onclick=` attributes, which a Content Security Policy blocks (see [Security](security.md)). Rails' Stimulus controllers map to small scripts that attach to `data-` attributes, or to a library such as Alpine.js loaded the same way.

| Rails | Ocre |
|---|---|
| Turbo Drive | `hx-boost="true"` (on in the layout) |
| Turbo Frames | `hx-get`/`hx-post` with `hx-target`, and the `Htmx` extractor |
| Turbo Streams | `hx-swap-oob` fragments; `ocre::realtime` over WebSockets |
| `data-turbo-method`, `data-turbo-confirm` | a `<form method="post">` button, `hx-confirm` |
| `request.js` with the CSRF header | htmx requests; no token needed |
| Stimulus | script files in `public/`, or Alpine.js |
| Import maps, jsbundling | see [Assets](assets.md) |

## See also

- [Views, helpers and forms](views.md): templates, layouts, partials and forms
- [Controllers and routing](controllers.md): handlers and extractors
- [Realtime](realtime.md): live updates for every visitor
- The [htmx reference](https://htmx.org/reference/) for every attribute
