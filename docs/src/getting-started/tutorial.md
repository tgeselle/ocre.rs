# Tutorial: a blog

This tutorial builds a small blog with Ocre, step by step: posts and comments stored in D1, validations, flash messages, accounts with login, protected pages, and a deploy to the Cloudflare Workers free plan.

## Before you start

- The tools from [Installation](installation.md): Rust through rustup with the `wasm32-unknown-unknown` target, Node.js 20 or newer, and the `ocre` CLI (`ocre --version` prints `ocre 0.1.0`).
- About 20 minutes. Nothing here needs a Cloudflare account until [Deploy](#deploy); the whole app runs locally.
- `curl`, to follow along from a terminal. A browser works as well: every page below is a normal HTML page at `http://localhost:8787`.

The commands and outputs on this page come from a real run, except [Deploy](#deploy), which is described from the CLI's code. Outputs that change from run to run (dates, durations, random tokens, cookie values) will differ on your machine.

## Create the app

```sh
ocre new blog --yes
cd blog
```

```text
  create  blog/Cargo.toml
  create  blog/wrangler.toml
  create  blog/rust-toolchain.toml
  create  blog/.gitignore
  create  blog/AGENTS.md
  create  blog/migrations/.gitkeep
  create  blog/public/robots.txt
  create  blog/src/lib.rs
  create  blog/templates/layout.html
  create  blog/templates/home.html
  create  blog/.dev.vars

Next:
  cd blog
  ocre dev
  ocre deploy
```

`--yes` skips the guided setup and keeps its defaults: a full-stack app (HTML pages), the empty starter, no Git repository, no Cloudflare login. Without `--yes`, in a terminal, `ocre new blog` asks those questions instead (see [Installation](installation.md#create-an-app-with-the-guided-setup)). Add `--git` to run `git init`.

The app is a Rust crate compiled to WebAssembly and run as one Cloudflare Worker. `src/lib.rs` is the entry point and the router:

```rust
// src/lib.rs
use askama::Template;
use axum::{Router, response::Html, routing::get};
use ocre::{Ctx, Result, render};
use worker::{Context, Env, HttpRequest, event};

// ocre:modules

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> worker::Result<worker::web_sys::Response> {
    ocre::serve(routes(), req, env).await
}

fn routes() -> Router<Ctx> {
    Router::new()
        .route("/", get(home))
        .route("/up", get(up))
        // ocre:routes
}

#[derive(Template)]
#[template(path = "home.html")]
struct HomeView;

async fn home() -> Result<Html<String>> {
    render(&HomeView)
}

/// Health check for uptime monitors and load balancers, like Rails' `/up`:
/// 200 `OK` whenever the Worker runs.
async fn up() -> &'static str {
    "OK"
}
```

`AGENTS.md` summarizes the app's conventions and commands; read it if you work on the app with an AI agent.

## Generate the Post resource

A scaffold generates a model, its migration, the HTML pages and their routes, like `rails generate scaffold`:

```sh
ocre g scaffold Post title:string body:text published:boolean
```

```text
  create  src/models/mod.rs
  create  migrations/0001_create_posts.sql
  create  src/models/post.rs
  create  src/posts.rs
  create  templates/posts/index.html
  create  templates/posts/show.html
  create  templates/posts/new.html
  create  templates/posts/edit.html
  create  templates/posts/_form.html
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/posts
```

| File | Role |
|---|---|
| `migrations/0001_create_posts.sql` | The `posts` table |
| `src/models/post.rs` | The model: `Post` (a row), `NewPost` and `PostChanges` (create and update inputs), `validate()`, and the queries `all`, `count`, `find`, `find_many`, `create`, `update`, `delete` |
| `src/posts.rs` | The controller: form parsing, the seven handlers and `routes()` |
| `templates/posts/*.html` | askama templates for the list, the post, the new and edit forms |
| `src/lib.rs` | Updated: `mod posts;`, `mod models;` and `.merge(posts::routes())` |

The migration is plain SQLite:

```sql
-- migrations/0001_create_posts.sql
CREATE TABLE posts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    published INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

A boolean is an `INTEGER` 0/1 in SQLite; the model reads it back as `bool`. Everything the scaffold wrote is ordinary Rust and HTML in your app, so you change it by editing it, as the next sections do. See [Generators](../reference/generators.md#ocre-g-scaffold) for every option and [Field types](../reference/field-types.md) for the types.

## Run the app

Apply the migration to the local database, then start the development server:

```sh
ocre migrate
```

```text
...
Migrations to be applied:
┌───────────────────────┐
│ name                  │
├───────────────────────┤
│ 0001_create_posts.sql │
└───────────────────────┘
...
🌀 Executing on local database blog (DB) from .wrangler/state/v3/d1:
🌀 To execute on your remote database, add a --remote flag to your wrangler command.
🚣 2 commands executed successfully.
┌───────────────────────┬────────┐
│ name                  │ status │
├───────────────────────┼────────┤
│ 0001_create_posts.sql │ ✅     │
└───────────────────────┴────────┘
```

The local database is a SQLite file under `.wrangler/state/`, kept between runs. `ocre dev` applies pending migrations itself, so running `ocre migrate` first is optional:

```sh
ocre dev
```

```text
...
✅ No migrations to apply!
...
[custom build] Running: cargo install -q "worker-build@^0.8" && worker-build ${OCRE_BUILD:---release}
[custom build] [INFO]: 🎯  Checking for the Wasm target...
[custom build] [INFO]: 🌀  Compiling to Wasm...
[custom build]    Compiling unicode-ident v1.0.26
...
[custom build]     Finished `dev` profile [unoptimized + debuginfo] target(s) in 36.09s
...
Using secrets defined in .dev.vars
Your Worker has access to the following bindings:
Binding                                                 Resource                  Mode
env.DB (blog)                                           D1 Database               local
env.MAIL_FROM ("blog <noreply@example.com>")            Environment Variable      local
env.SECRET_KEY_BASE ("(hidden)")                        Environment Variable      local
env.MAIL_ADAPTER ("(hidden)")                           Environment Variable      local

⎔ Starting local server...
...
[wrangler:info] Ready on http://localhost:8787
```

The first build compiles every dependency to WebAssembly and installs `worker-build`; it takes a minute or two. Later builds take seconds, and `ocre dev` rebuilds when a file in `src/` changes. Leave it running in its own terminal and stop it with Ctrl-C. `--port N` serves on another port.

Open `http://localhost:8787/posts` in a browser, or use `curl` from another terminal:

```sh
curl -i http://localhost:8787/posts
```

```text
HTTP/1.1 200 OK
Transfer-Encoding: chunked
Content-Type: text/html; charset=utf-8
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0

<!doctype html>
...
<h1>Posts</h1>
...
<p><a href="/posts/new">New post</a></p>
...
```

Every response carries the security headers above; `ocre::serve` adds them (see [Sessions, flash and security](../guides/security.md)). Create a post with the form at `/posts/new`, or post the same form with `curl`:

```sh
curl -i http://localhost:8787/posts -d 'title=Hello+Ocre&body=My+first+post&published=true'
```

```text
HTTP/1.1 303 See Other
Content-Length: 0
Location: /posts/1
Set-Cookie: _ocre_session=3VsAxzOOtqZzbpER5LvexSh2pyxdC9nX0nilcTIdCiMmYCt05DfyP63FkRHY6DNnJlIlxvRl%2Fbf+7KFvBKXwgO53qGacK4zXv5XjsSFA0C5MZw%3D%3D; HttpOnly; SameSite=Lax; Path=/
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0
```

The post is saved and the browser is redirected to its page, `/posts/1`:

```html
...
<h1>Post 1</h1>
...
<dl>
  <dt>Title</dt><dd>Hello Ocre</dd>
  <dt>Body</dt><dd>My first post</dd>
  <dt>Published</dt><dd>true</dd>
  <dt>Created at</dt><dd>2026-09-29 04:37:00</dd>
  <dt>Updated at</dt><dd>2026-09-29 04:37:00</dd>
</dl>
```

The session cookie in the answer carries the flash message shown on the next page (see [Flash messages](#flash-messages)). The server terminal logs one line per request, such as `[wrangler:info] GET /account 200 OK (18ms)`.

The scaffold's routes:

| Route | Handler | Page |
|---|---|---|
| `GET /posts` | `index` | The list, newest first (`?limit=` and `?offset=` paginate, 50 by default) |
| `GET /posts/new` | `new` | The new-post form |
| `POST /posts` | `create` | Creates, then redirects to the post |
| `GET /posts/{id}` | `show` | One post |
| `GET /posts/{id}/edit` | `edit` | The edit form |
| `POST /posts/{id}` | `update` | Updates, then redirects to the post |
| `POST /posts/{id}/delete` | `delete` | Deletes, then redirects to the list |

HTML forms can only send GET and POST, so updates and deletes are POST routes.

## Add comments

Comments belong to a post. The `references` type adds a `post_id` column with a foreign key:

```sh
ocre g scaffold Comment author:string body:text post:references
```

```text
  create  migrations/0002_create_comments.sql
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

```sql
-- migrations/0002_create_comments.sql
CREATE TABLE comments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    author TEXT NOT NULL,
    body TEXT NOT NULL,
    post_id INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX index_comments_on_post_id ON comments (post_id);
```

`ON DELETE CASCADE` deletes a post's comments with it. The generator wrote both sides of the association: `comment.post(&ctx)` in `src/models/comment.rs`, and in `src/models/post.rs`:

```rust
// src/models/post.rs (generated)
impl Post {
    // ocre:associations
    /// Comments of this post, newest first.
    pub async fn comments(&self, ctx: &Ctx, page: ocre::Page) -> Result<Vec<crate::models::comment::Comment>> {
        ctx.db()?
            .all(
                "SELECT * FROM comments WHERE post_id = ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3",
                params![self.id, page.limit, page.offset],
            )
            .await
    }
}
```

`comment::create` also checks that the post exists (the error "Post must exist") before inserting. Stop `ocre dev` (Ctrl-C) and apply the migration; starting `ocre dev` again would apply it as well:

```sh
ocre migrate
```

```text
...
🚣 3 commands executed successfully.
┌──────────────────────────┬────────┐
│ name                     │ status │
├──────────────────────────┼────────┤
│ 0002_create_comments.sql │ ✅     │
└──────────────────────────┴────────┘
```

The scaffold also generated a standalone CRUD at `/comments`, where the post is a number field. A blog shows comments under their post instead, which the next section does.

## Show comments on the post page

Three changes in `src/posts.rs`: the `show` handler loads the post's comments with `post.comments(&ctx, page)`, `ShowView` carries them plus a comment form and its errors, and a new route `POST /posts/{id}/comments` creates a comment for the post in the URL. Replace the whole file:

```rust,check
// src/posts.rs
//! Posts pages (HTML). Generated by `ocre g scaffold Post title:string body:text published:boolean`.
//! Queries and rules live in the model, `crate::models::post`.

use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use ocre::{Ctx, Error, FieldError, Flash, OptionExt, Page, Result, Session, Validator, render};
use serde::Deserialize;

use crate::models::comment::{self, Comment, NewComment};
use crate::models::post::{self, NewPost, Post, PostChanges};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/posts", get(index).post(create))
        .route("/posts/new", get(new))
        .route("/posts/{id}", get(show).post(update))
        .route("/posts/{id}/edit", get(edit))
        .route("/posts/{id}/delete", post(delete))
        .route("/posts/{id}/comments", post(create_comment))
}

/// What the new and edit forms submit, as typed: numbers stay text until
/// validated, so a typo shows a field error instead of a failed request.
/// A missing field is empty (and unchecked for checkboxes).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PostForm {
    pub title: String,
    pub body: String,
    /// Unchecked checkboxes are not submitted.
    pub published: bool,
}

impl PostForm {
    fn from_record(record: &Post) -> Self {
        Self {
            title: record.title.clone(),
            body: record.body.clone(),
            published: record.published,
        }
    }

    /// Parses the text and runs the model's checks, so the form shows every
    /// error at once (database checks run in `create`).
    fn to_new(&self) -> Result<NewPost> {
        let mut v = Validator::new();
        let new = NewPost {
            title: self.title.clone(),
            body: self.body.clone(),
            published: self.published,
        };
        v.merge(new.validate()).finish()?;
        Ok(new)
    }

    fn to_changes(&self) -> Result<PostChanges> {
        let mut v = Validator::new();
        let changes = PostChanges {
            title: Some(self.title.clone()),
            body: Some(self.body.clone()),
            published: Some(self.published),
        };
        v.merge(changes.validate()).finish()?;
        Ok(changes)
    }
}

/// The comment form on a post's page; the post comes from the URL.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CommentForm {
    pub author: String,
    pub body: String,
}

#[derive(Template)]
#[template(path = "posts/index.html")]
struct IndexView {
    flash: Flash,
    posts: Vec<Post>,
}

#[derive(Template)]
#[template(path = "posts/show.html")]
struct ShowView {
    flash: Flash,
    post: Post,
    comments: Vec<Comment>,
    comment_form: CommentForm,
    errors: Vec<FieldError>,
}

#[derive(Template)]
#[template(path = "posts/new.html")]
struct NewView {
    form: PostForm,
    errors: Vec<FieldError>,
}

#[derive(Template)]
#[template(path = "posts/edit.html")]
struct EditView {
    id: i64,
    form: PostForm,
    errors: Vec<FieldError>,
}

async fn index(State(ctx): State<Ctx>, flash: Flash, page: Page) -> Result<Html<String>> {
    render(&IndexView { flash, posts: post::all(&ctx, page).await? })
}

async fn show(State(ctx): State<Ctx>, flash: Flash, page: Page, Path(id): Path<i64>) -> Result<Html<String>> {
    let post = post::find(&ctx, id).await?.or_404()?;
    let comments = post.comments(&ctx, page).await?;
    render(&ShowView { flash, post, comments, comment_form: CommentForm::default(), errors: vec![] })
}

async fn new() -> Result<Html<String>> {
    render(&NewView { form: PostForm::default(), errors: vec![] })
}

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

async fn edit(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Html<String>> {
    let record = post::find(&ctx, id).await?.or_404()?;
    render(&EditView { id, form: PostForm::from_record(&record), errors: vec![] })
}

async fn update(
    State(ctx): State<Ctx>,
    session: Session,
    Path(id): Path<i64>,
    Form(form): Form<PostForm>,
) -> Result<Response> {
    let updated = match form.to_changes() {
        Ok(changes) => post::update(&ctx, id, changes).await,
        Err(err) => Err(err),
    };
    match updated {
        Ok(record) => {
            let record = record.or_404()?;
            session.flash("notice", "Post was successfully updated.")?;
            Ok(Redirect::to(&format!("/posts/{}", record.id)).into_response())
        }
        Err(Error::Invalid(errors)) => {
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&EditView { id, form, errors })?).into_response())
        }
        Err(err) => Err(err),
    }
}

async fn delete(State(ctx): State<Ctx>, session: Session, Path(id): Path<i64>) -> Result<Redirect> {
    if !post::delete(&ctx, id).await? {
        return Err(Error::NotFound);
    }
    session.flash("notice", "Post was successfully destroyed.")?;
    Ok(Redirect::to("/posts"))
}

/// Adds a comment to the post, then shows the post again. Invalid input
/// re-renders the post page with the messages and the typed values.
async fn create_comment(
    State(ctx): State<Ctx>,
    session: Session,
    page: Page,
    Path(id): Path<i64>,
    Form(form): Form<CommentForm>,
) -> Result<Response> {
    let post = post::find(&ctx, id).await?.or_404()?;
    let new = NewComment { author: form.author.clone(), body: form.body.clone(), post_id: post.id };
    match comment::create(&ctx, new).await {
        Ok(_) => {
            session.flash("notice", "Comment was successfully created.")?;
            Ok(Redirect::to(&format!("/posts/{id}")).into_response())
        }
        Err(Error::Invalid(errors)) => {
            let comments = post.comments(&ctx, page).await?;
            let view = ShowView { flash: Flash::default(), post, comments, comment_form: form, errors };
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&view)?).into_response())
        }
        Err(err) => Err(err),
    }
}
```

`Page` is an extractor that reads `?limit=` and `?offset=` from the query string (50 comments by default, at most 100), so `/posts/1?offset=50` shows the next comments. `create_comment` builds a `NewComment` itself, taking the post id from the URL rather than from the form, and calls the model's `create`, which runs the same validations as the `/comments` pages.

Then list the comments and add the form at the end of `templates/posts/show.html`. The full template:

```html
{% extends "layout.html" %}

{% block title %}Post {{ post.id }}{% endblock %}

{% block content %}
<h1>Post {{ post.id }}</h1>
{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
{% if let Some(alert) = flash.alert() %}<p class="alert">{{ alert }}</p>{% endif %}

<dl>
  <dt>Title</dt><dd>{{ post.title }}</dd>
  <dt>Body</dt><dd>{{ post.body }}</dd>
  <dt>Published</dt><dd>{{ post.published }}</dd>
  <dt>Created at</dt><dd>{{ post.created_at }}</dd>
  <dt>Updated at</dt><dd>{{ post.updated_at }}</dd>
</dl>

<p><a href="/posts/{{ post.id }}/edit">Edit</a> · <a href="/posts">Back</a></p>

<form action="/posts/{{ post.id }}/delete" method="post" onsubmit="return confirm('Delete this post?')">
  <button type="submit">Delete post</button>
</form>

<h2>Comments</h2>
{% for comment in comments %}
<p><strong>{{ comment.author }}</strong>: {{ comment.body }}</p>
{% else %}
<p>No comments yet.</p>
{% endfor %}

<h3>Add a comment</h3>
<form action="/posts/{{ post.id }}/comments" method="post">
{% if !errors.is_empty() %}
  <ul class="errors">
    {% for error in errors %}<li>{{ error.full_message() }}</li>{% endfor %}
  </ul>
{% endif %}
  <label>Author <input name="author" value="{{ comment_form.author }}" required></label>
  <label>Body <textarea name="body" rows="3" required>{{ comment_form.body }}</textarea></label>
  <button type="submit">Add comment</button>
</form>
{% endblock %}
```

askama templates are compiled into the Worker, so a template that uses a field `ShowView` lacks is a compile error, not a runtime surprise. `{{ ... }}` escapes HTML, so comment text cannot inject markup.

Start `ocre dev` again. While it runs, it rebuilds when a file in `src/` changes; restart it after changing only a template. Add a comment:

```sh
curl -c cookies.txt -b cookies.txt -L http://localhost:8787/posts/1/comments -d 'author=Ada&body=Nice+post'
```

```html
...
<h1>Post 1</h1>
<p class="notice">Comment was successfully created.</p>
...
<h2>Comments</h2>

<p><strong>Ada</strong>: Nice post</p>


<h3>Add a comment</h3>
<form action="/posts/1/comments" method="post">

  <label>Author <input name="author" value="" required></label>
  <label>Body <textarea name="body" rows="3" required></textarea></label>
  <button type="submit">Add comment</button>
</form>
```

`-c`/`-b` keep the session cookie between requests, as a browser does; `-L` follows the redirect. An empty comment re-renders the post page with status 422 and the messages:

```sh
curl -s -o /dev/null -w '%{http_code}\n' http://localhost:8787/posts/1/comments -d 'author=&body='
curl -s http://localhost:8787/posts/1/comments -d 'author=&body=' | grep -A2 'class="errors"'
```

```text
422
  <ul class="errors">
    <li>Author can&#39;t be blank</li><li>Body can&#39;t be blank</li>
  </ul>
```

A comment for a post that does not exist (`POST /posts/99/comments`) answers 404: `.or_404()?` turns the missing post into `Error::NotFound`.

## Add a validation

Rules about data live in the model's `validate()` functions, in `src/models/post.rs`. Limit titles to 100 characters by chaining `max_length` after `required`, in both `NewPost` (create) and `PostChanges` (update):

```rust
// src/models/post.rs
impl NewPost {
    /// Checks that need no database; `create` adds uniqueness and references.
    pub fn validate(&self) -> Validator {
        let mut v = Validator::new();
        v.required("title", &self.title).max_length("title", &self.title, 100);
        v.required("body", &self.body);
        v
    }
}

impl PostChanges {
    /// Checks the fields being changed; `update` adds the database checks.
    pub fn validate(&self) -> Validator {
        let mut v = Validator::new();
        if let Some(title) = &self.title {
            v.required("title", title).max_length("title", title, 100);
        }
        if let Some(body) = &self.body {
            v.required("body", body);
        }
        v
    }
}
```

`Validator` collects every error before answering; `finish()` turns them into `Error::Invalid`, status 422. The form handlers call `validate()` through `PostForm::to_new` and `to_changes`, and the model's `create` and `update` call it again, so a JSON API or a job going through the model gets the same rule. Try a 101-character title:

```sh
curl -s http://localhost:8787/posts -d "title=$(printf 'x%.0s' $(seq 101))&body=Long"
```

```html
...
<h1>New post</h1>

<form action="/posts" method="post">

  <ul class="errors">
    <li>Title is too long (maximum is 100 characters)</li>
  </ul>

  <label>Title <input name="title" value="xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx" required></label>
...
```

The `create` handler catches `Error::Invalid(errors)` and renders the new-post form again with status 422 (`curl -s -o /dev/null -w '%{http_code}\n' ...` prints it), the messages and the values that were typed. The same happens with empty fields ("Title can't be blank"). Other rules (`min_length`, `range`, `email`, `inclusion`, `date`, `check` for anything custom) are in [Validations](../guides/validations.md).

## Flash messages

After a successful create, update or delete, the scaffold redirects and the next page shows a one-time message such as "Post was successfully created.". Two pieces make it work:

- Before redirecting, the handler stores the message in the session: `session.flash("notice", "Post was successfully created.")?;` (the `session: Session` argument).
- The next page takes `flash: Flash` as an argument and passes it to its template, which shows `flash.notice()` and `flash.alert()`:

```html
{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
{% if let Some(alert) = flash.alert() %}<p class="alert">{{ alert }}</p>{% endif %}
```

Extracting `Flash` removes the messages from the session, so they show once:

```sh
curl -s -c cookies.txt -b cookies.txt -L http://localhost:8787/posts -d 'title=Second+post&body=With+a+flash' | grep 'class="notice"'
curl -s -c cookies.txt -b cookies.txt http://localhost:8787/posts/2 | grep 'class="notice"'
```

```text
<p class="notice">Post was successfully created.</p>
```

The second request prints nothing: the message was consumed by the first page. The session is an encrypted cookie (`_ocre_session`, AES-256-GCM with a key derived from `SECRET_KEY_BASE`), so a flash costs no database or KV operation. A page that does not take `Flash` leaves the message for the next page that does. `notice` and `alert` are conventions; any kind works with `flash.get("kind")`. See [Sessions, flash and security](../guides/security.md).

## Add authentication

Stop `ocre dev`, then generate users, sign-up, login (password or emailed magic link), logout, password reset, and a JSON API with JWTs and API keys, all as code in the app:

```sh
ocre g auth
```

```text
  create  migrations/0003_create_users.sql
  create  migrations/0004_create_auth_tokens.sql
  create  migrations/0005_create_api_keys.sql
  create  src/models/user.rs
  create  src/models/api_key.rs
  create  src/models/auth_token.rs
  create  src/auth_api.rs
  create  src/auth.rs
  create  src/registrations.rs
  create  src/sessions.rs
  create  src/passwords.rs
  create  templates/auth/signup.html
  create  templates/auth/login.html
  create  templates/auth/account.html
  create  templates/auth/magic_link_new.html
  create  templates/auth/magic_link_show.html
  create  templates/auth/password_new.html
  create  templates/auth/password_edit.html
  update  src/models/mod.rs
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  open http://localhost:8787/signup
```

```sh
ocre migrate
```

```text
...
Migrations to be applied:
┌─────────────────────────────┐
│ name                        │
├─────────────────────────────┤
│ 0003_create_users.sql       │
├─────────────────────────────┤
│ 0004_create_auth_tokens.sql │
├─────────────────────────────┤
│ 0005_create_api_keys.sql    │
└─────────────────────────────┘
...
┌─────────────────────────────┬────────┐
│ name                        │ status │
├─────────────────────────────┼────────┤
│ 0003_create_users.sql       │ ✅     │
├─────────────────────────────┼────────┤
│ 0004_create_auth_tokens.sql │ ✅     │
├─────────────────────────────┼────────┤
│ 0005_create_api_keys.sql    │ ✅     │
└─────────────────────────────┴────────┘
```

Start `ocre dev` again so the new modules are built. The HTML pages:

| Page | Purpose |
|---|---|
| `GET/POST /signup` | Create an account (email, password of 8 to 128 characters), then sign in |
| `GET /account` | An example protected page |
| `GET/POST /login`, `POST /logout` | Password login and logout |
| `GET/POST /magic_link` | Email a sign-in link |
| `GET/POST /magic_link/{token}` | The emailed link: a page with a button that signs in |
| `GET /passwords/new`, `POST /passwords`, `GET/POST /passwords/{token}` | Password reset by emailed link |

### Sign up, log out, log in

```sh
curl -si -c cookies.txt -b cookies.txt http://localhost:8787/signup -d 'email=ada@example.com&password=correct-horse' | grep -i '^http\|^location'
```

```text
HTTP/1.1 303 See Other
Location: /
```

The account exists and the session holds its user id. Passwords are hashed with PBKDF2-HMAC-SHA256 (100,000 iterations) through WebCrypto, about 5 ms of CPU per hash. The home page does not show flash messages, so the "Welcome! Your account is ready." notice waits for the next page that does, here `/account`:

```sh
curl -s -c cookies.txt -b cookies.txt http://localhost:8787/account | grep -E 'class="notice"|<dd>'
```

```text
<p class="notice">Welcome! Your account is ready.</p>
  <dt>Email</dt><dd>ada@example.com</dd>
  <dt>Member since</dt><dd>2026-09-29 04:38:39</dd>
```

Log out, then try the protected page again:

```sh
curl -si -c cookies.txt -b cookies.txt -X POST http://localhost:8787/logout | grep -i '^http\|^location'
curl -si -c cookies.txt -b cookies.txt http://localhost:8787/account | grep -i '^http\|^location'
curl -s -c cookies.txt -b cookies.txt http://localhost:8787/login | grep 'class="alert"'
```

```text
HTTP/1.1 303 See Other
Location: /
HTTP/1.1 303 See Other
Location: /login
<p class="alert">Please log in to continue.</p>
```

A wrong password re-renders the login form with status 422 and `Invalid email or password.`. The right one signs in and returns to the page that asked for the login:

```sh
curl -si -c cookies.txt -b cookies.txt http://localhost:8787/login -d 'email=ada@example.com&password=correct-horse' | grep -i '^http\|^location'
```

```text
HTTP/1.1 303 See Other
Location: /account
```

### Magic links and password resets in development

The magic-link and password-reset forms send email with `ocre::mail::send`. `ocre new` wrote `MAIL_ADAPTER=log` to `.dev.vars`, so in `ocre dev` no email leaves your machine: each one is printed in the `ocre dev` terminal instead. Ask for a sign-in link:

```sh
curl -si http://localhost:8787/magic_link -d 'email=ada@example.com' | grep -i '^http\|^location'
```

```text
HTTP/1.1 303 See Other
Location: /login
```

The `ocre dev` terminal shows the email:

```text
[ocre mail] not sent (MAIL_ADAPTER = "log")
From: blog <noreply@example.com>
To: ada@example.com
Subject: Your sign-in link

Open this link within 15 minutes to sign in:

http://localhost:8787/magic_link/D5bEcXxUQD8eo_hu4KImsWnB34t_tInbOs1Z75PYwAc

If you did not ask for it, ignore this email.

[ocre mail] HTML version:
<p><a href="http://localhost:8787/magic_link/D5bEcXxUQD8eo_hu4KImsWnB34t_tInbOs1Z75PYwAc">Sign in</a> (valid 15 minutes).</p><p>If you did not ask for it, ignore this email.</p>
[ocre mail] end
[wrangler:info] POST /magic_link 303 See Other (18ms)
```

Opening the link shows a page with a "Sign in" button; the button POSTs to the same URL, which signs in and redirects to `/`. Mail scanners that follow links therefore cannot use the token. A token works once, for 15 minutes: posting it again redirects to `/magic_link` with "That sign-in link is invalid or has expired.". `/passwords/new` works the same way and prints a "Reset your password" email. For an unknown address both forms answer exactly as for a known one, so they do not reveal which emails have accounts.

## Protect the post pages

Anyone can still create, edit and delete posts. The `CurrentUser` extractor from `src/auth.rs` requires a signed-in user: it redirects other visitors to `/login` (remembering the page for GET requests) and sets the alert "Please log in to continue.". Add it as the first argument of `new`, `create`, `edit`, `update` and `delete`; `index`, `show` and `create_comment` stay public. The whole `src/posts.rs`:

```rust,check
// src/posts.rs
//! Posts pages (HTML). Generated by `ocre g scaffold Post title:string body:text published:boolean`.
//! Queries and rules live in the model, `crate::models::post`.

use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use ocre::{Ctx, Error, FieldError, Flash, OptionExt, Page, Result, Session, Validator, render};
use serde::Deserialize;

use crate::auth::CurrentUser;
use crate::models::comment::{self, Comment, NewComment};
use crate::models::post::{self, NewPost, Post, PostChanges};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/posts", get(index).post(create))
        .route("/posts/new", get(new))
        .route("/posts/{id}", get(show).post(update))
        .route("/posts/{id}/edit", get(edit))
        .route("/posts/{id}/delete", post(delete))
        .route("/posts/{id}/comments", post(create_comment))
}

/// What the new and edit forms submit, as typed: numbers stay text until
/// validated, so a typo shows a field error instead of a failed request.
/// A missing field is empty (and unchecked for checkboxes).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PostForm {
    pub title: String,
    pub body: String,
    /// Unchecked checkboxes are not submitted.
    pub published: bool,
}

impl PostForm {
    fn from_record(record: &Post) -> Self {
        Self {
            title: record.title.clone(),
            body: record.body.clone(),
            published: record.published,
        }
    }

    /// Parses the text and runs the model's checks, so the form shows every
    /// error at once (database checks run in `create`).
    fn to_new(&self) -> Result<NewPost> {
        let mut v = Validator::new();
        let new = NewPost {
            title: self.title.clone(),
            body: self.body.clone(),
            published: self.published,
        };
        v.merge(new.validate()).finish()?;
        Ok(new)
    }

    fn to_changes(&self) -> Result<PostChanges> {
        let mut v = Validator::new();
        let changes = PostChanges {
            title: Some(self.title.clone()),
            body: Some(self.body.clone()),
            published: Some(self.published),
        };
        v.merge(changes.validate()).finish()?;
        Ok(changes)
    }
}

/// The comment form on a post's page; the post comes from the URL.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CommentForm {
    pub author: String,
    pub body: String,
}

#[derive(Template)]
#[template(path = "posts/index.html")]
struct IndexView {
    flash: Flash,
    posts: Vec<Post>,
}

#[derive(Template)]
#[template(path = "posts/show.html")]
struct ShowView {
    flash: Flash,
    post: Post,
    comments: Vec<Comment>,
    comment_form: CommentForm,
    errors: Vec<FieldError>,
}

#[derive(Template)]
#[template(path = "posts/new.html")]
struct NewView {
    form: PostForm,
    errors: Vec<FieldError>,
}

#[derive(Template)]
#[template(path = "posts/edit.html")]
struct EditView {
    id: i64,
    form: PostForm,
    errors: Vec<FieldError>,
}

async fn index(State(ctx): State<Ctx>, flash: Flash, page: Page) -> Result<Html<String>> {
    render(&IndexView { flash, posts: post::all(&ctx, page).await? })
}

async fn show(State(ctx): State<Ctx>, flash: Flash, page: Page, Path(id): Path<i64>) -> Result<Html<String>> {
    let post = post::find(&ctx, id).await?.or_404()?;
    let comments = post.comments(&ctx, page).await?;
    render(&ShowView { flash, post, comments, comment_form: CommentForm::default(), errors: vec![] })
}

async fn new(_: CurrentUser) -> Result<Html<String>> {
    render(&NewView { form: PostForm::default(), errors: vec![] })
}

async fn create(
    _: CurrentUser,
    State(ctx): State<Ctx>,
    session: Session,
    Form(form): Form<PostForm>,
) -> Result<Response> {
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

async fn edit(_: CurrentUser, State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Html<String>> {
    let record = post::find(&ctx, id).await?.or_404()?;
    render(&EditView { id, form: PostForm::from_record(&record), errors: vec![] })
}

async fn update(
    _: CurrentUser,
    State(ctx): State<Ctx>,
    session: Session,
    Path(id): Path<i64>,
    Form(form): Form<PostForm>,
) -> Result<Response> {
    let updated = match form.to_changes() {
        Ok(changes) => post::update(&ctx, id, changes).await,
        Err(err) => Err(err),
    };
    match updated {
        Ok(record) => {
            let record = record.or_404()?;
            session.flash("notice", "Post was successfully updated.")?;
            Ok(Redirect::to(&format!("/posts/{}", record.id)).into_response())
        }
        Err(Error::Invalid(errors)) => {
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&EditView { id, form, errors })?).into_response())
        }
        Err(err) => Err(err),
    }
}

async fn delete(_: CurrentUser, State(ctx): State<Ctx>, session: Session, Path(id): Path<i64>) -> Result<Redirect> {
    if !post::delete(&ctx, id).await? {
        return Err(Error::NotFound);
    }
    session.flash("notice", "Post was successfully destroyed.")?;
    Ok(Redirect::to("/posts"))
}

/// Adds a comment to the post, then shows the post again. Invalid input
/// re-renders the post page with the messages and the typed values.
async fn create_comment(
    State(ctx): State<Ctx>,
    session: Session,
    page: Page,
    Path(id): Path<i64>,
    Form(form): Form<CommentForm>,
) -> Result<Response> {
    let post = post::find(&ctx, id).await?.or_404()?;
    let new = NewComment { author: form.author.clone(), body: form.body.clone(), post_id: post.id };
    match comment::create(&ctx, new).await {
        Ok(_) => {
            session.flash("notice", "Comment was successfully created.")?;
            Ok(Redirect::to(&format!("/posts/{id}")).into_response())
        }
        Err(Error::Invalid(errors)) => {
            let comments = post.comments(&ctx, page).await?;
            let view = ShowView { flash: Flash::default(), post, comments, comment_form: form, errors };
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&view)?).into_response())
        }
        Err(err) => Err(err),
    }
}
```

`_: CurrentUser` only requires the login; write `CurrentUser(user): CurrentUser` to use the `User` (for example to store `user.id` on the post, after a migration that adds the column). `OptionalUser(user): OptionalUser` gives an `Option<User>` for pages that change with the login but stay public. Extractors that read the request body (`Form`) must come last; `CurrentUser` reads only the cookie.

```sh
rm cookies.txt
curl -si -c cookies.txt -b cookies.txt http://localhost:8787/posts/new | grep -i '^http\|^location'
curl -si http://localhost:8787/posts -d 'title=x&body=y' | grep -i '^http\|^location'
curl -si -c cookies.txt -b cookies.txt http://localhost:8787/login -d 'email=ada@example.com&password=correct-horse' | grep -i '^http\|^location'
curl -s -c cookies.txt -b cookies.txt http://localhost:8787/posts/new | grep '<h1>'
```

```text
HTTP/1.1 303 See Other
Location: /login
HTTP/1.1 303 See Other
Location: /login
HTTP/1.1 303 See Other
Location: /posts/new
<h1>New post</h1>
```

`GET /posts/1` still answers 200 without a login. The standalone `/comments` pages from the Comment scaffold are not protected; protect them the same way in `src/comments.rs`, or remove `mod comments;` and `.merge(comments::routes())` from `src/lib.rs` if the post page is the only way to comment.

## List the routes

`ocre routes` reads the routes from `src/lib.rs` and the modules it merges, without building:

```sh
ocre routes
```

```text
METHOD  PATH                   HANDLER
GET     /                      home
GET     /account               registrations::show
GET     /api/auth/keys         auth_api::list_keys
POST    /api/auth/keys         auth_api::create_key
DELETE  /api/auth/keys/{id}    auth_api::revoke_key
GET     /api/auth/me           auth_api::me
POST    /api/auth/signup       auth_api::signup
POST    /api/auth/token        auth_api::token
GET     /comments              comments::index
POST    /comments              comments::create
GET     /comments/new          comments::new
GET     /comments/{id}         comments::show
POST    /comments/{id}         comments::update
POST    /comments/{id}/delete  comments::delete
GET     /comments/{id}/edit    comments::edit
GET     /login                 sessions::new
POST    /login                 sessions::create
POST    /logout                sessions::destroy
GET     /magic_link            sessions::new_magic_link
POST    /magic_link            sessions::create_magic_link
GET     /magic_link/{token}    sessions::show_magic_link
POST    /magic_link/{token}    sessions::use_magic_link
POST    /passwords             passwords::create
GET     /passwords/new         passwords::new
GET     /passwords/{token}     passwords::edit
POST    /passwords/{token}     passwords::update
GET     /posts                 posts::index
POST    /posts                 posts::create
GET     /posts/new             posts::new
GET     /posts/{id}            posts::show
POST    /posts/{id}            posts::update
POST    /posts/{id}/comments   posts::create_comment
POST    /posts/{id}/delete     posts::delete
GET     /posts/{id}/edit       posts::edit
GET     /signup                registrations::new
POST    /signup                registrations::create
GET     /up                    up
```

A filter keeps the routes whose method, path or handler contains it (`ocre routes posts`); `--json` returns them as a `routes` array.

## Deploy

Deploying needs a Cloudflare account (the free plan is enough) and the login of wrangler. The commands in this section were not run for this page: the behavior below is described from the CLI's code.

### Log in

```sh
ocre login
```

If wrangler already has a login (or `CLOUDFLARE_API_TOKEN` is set), nothing opens. Otherwise it runs `wrangler login`, which opens the browser to approve access. The command then prints `Logged in to Cloudflare as <email>`. A login with several accounts needs `account_id = "..."` in `wrangler.toml` (`ocre new --account-id` writes it).

### Email in production

The auth pages send email, and in production `.dev.vars` does not apply: with `MAIL_ADAPTER` unset, the magic-link and password-reset forms answer 500 and the log names the fix (`cannot send email: MAIL_ADAPTER is not set. ...`). Sign-up and password login do not send email and work without it. Before deploying, pick an adapter; on the free plan, [Resend](https://resend.com/docs/knowledge-base/account-quotas-and-limits) sends to any recipient (free: 100 emails a day, 3,000 a month, one domain, September 2026):

1. In `wrangler.toml`, under `[vars]`, uncomment `MAIL_ADAPTER = "resend"` and set `MAIL_FROM` to an address on a domain verified in Resend, for example `MAIL_FROM = "Blog <noreply@yourdomain.com>"`.
2. Store the API key as a secret: `npx wrangler secret put RESEND_API_KEY` (it prompts for the value). A Worker must exist before it can have secrets, so run this after the first `ocre deploy` if the Worker is new.

See [Email](../guides/email.md) for the `cloudflare` adapter and [Configuration](../reference/configuration.md#mail_adapter) for every variable.

### ocre deploy

```sh
ocre deploy
```

In order, `ocre deploy`:

1. Checks the locale files (when the app has translations) and the `wasm32-unknown-unknown` target, as `ocre dev` does.
2. Creates the Cloudflare resources `wrangler.toml` names that are missing: queues, KV namespaces without an `id`, R2 buckets. This blog uses none of them.
3. Asks `wrangler secret list` whether the Worker has `SECRET_KEY_BASE`. A new Worker has none, so a fresh random one is uploaded with the deploy (`--secrets-file`). An existing secret is never replaced, since that would sign every user out.
4. Looks up the D1 database named in `wrangler.toml` (`blog`). When it exists, applies the pending migrations to it (`--remote`), then deploys; the first time, deploys first (wrangler creates the database and binds it), then applies all migrations.
5. Deploys with `wrangler deploy`, which builds the Worker in release mode (optimized for size, slower to compile than `ocre dev`) and uploads it.

wrangler's own output is shown as it runs; the command then ends with:

```text
Created the SECRET_KEY_BASE secret on Cloudflare

https://blog.<your-subdomain>.workers.dev
```

The first line appears only on the deploy that created the secret. With `--json`, the result is `{"command": "deploy", "ok": true, "secret_created": true, "url": "https://blog.<your-subdomain>.workers.dev"}`. Run `ocre deploy` again after each change: later deploys migrate the database first, so the new code never runs against an old schema.

Free-plan limits that matter for this blog (September 2026, [Workers limits](https://developers.cloudflare.com/workers/platform/limits/)): 100,000 Worker requests a day and 10 ms of CPU per request. Page views cost well under 10 ms; a login or sign-up uses about half of it for the password hash. The login, sign-up and email routes have no rate limiting: before going public, add [Cloudflare rate limiting rules](https://developers.cloudflare.com/waf/rate-limiting-rules/) for `/login`, `/signup`, `/magic_link`, `/passwords` and `/api/auth/*`. [Free-plan limits](../reference/limits.md) lists the rest, including D1.

## Next steps

- [Models and migrations](../guides/models.md): queries, associations, changing columns with `ocre g migration`.
- [Validations](../guides/validations.md): every `Validator` rule and how errors reach forms and JSON.
- [Controllers, routing, views and htmx](../guides/controllers.md): handlers, templates, partial updates with htmx.
- [Authentication](../guides/authentication.md): `CurrentUser`, ownership checks, JWTs and API keys.
- [Email](../guides/email.md): mailers, Resend, Cloudflare Email Service, receiving email.
- [Background jobs and schedules](../guides/jobs.md): send email from a queue, run nightly tasks.
- [JSON APIs and GraphQL](../guides/json-apis.md): the same posts as a JSON API.
- [Deployment](../guides/deployment.md): environments, secrets, what `ocre deploy` creates.
- [CLI commands](../reference/cli.md) and [Generators](../reference/generators.md): every command and flag.
