# Authentication

This guide adds users to an Ocre app with `ocre g auth`: sign-up, login and logout pages, magic links and password resets by email, JWTs and API keys for JSON clients, then shows how to protect pages and endpoints and restrict records to their owner. All of it is generated app code you can read and change; the framework only provides small primitives (password hashing, tokens, JWTs).

## Before you start

- An app created with [`ocre new`](../reference/cli.md#ocre-new), full-stack or API-only (`--api`). `ocre g auth` runs once per app and refuses to run when a `User` model or a `users` table exists.
- The `SECRET_KEY_BASE` secret, used for session cookies and JWTs; `ocre new` writes one to `.dev.vars` (see [Sessions, flash and security](security.md#keys-and-rotation)).
- For magic links and password resets in production: a mail adapter (`MAIL_ADAPTER`, see [Email](email.md)). In `ocre dev` the emails are printed in the console.
- The outputs below come from an app created with `ocre new shop --starter blog`, served by `ocre dev` on the default port 8787.

## Generate authentication

```sh
ocre g auth
```

In a full-stack app:

```text
  create  migrations/0002_create_users.sql
  create  migrations/0003_create_auth_tokens.sql
  create  migrations/0004_create_api_keys.sql
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

Then apply the migrations and start the app:

```sh
ocre migrate
ocre dev
```

What each file holds, and which ones an API-only app (`ocre new --api`) gets:

| File | Full-stack | API-only | Contents |
|---|---|---|---|
| `migrations/*_create_users.sql` | yes | yes | `users`: `email` (unique, `COLLATE NOCASE`), `password_digest`, timestamps |
| `migrations/*_create_auth_tokens.sql` | yes | | Single-use emailed tokens: `user_id`, `purpose` (`password_reset` or `magic_link`), `digest` (unique), `expires_at` |
| `migrations/*_create_api_keys.sql` | yes | yes | `api_keys`: `user_id`, `name`, `digest` (unique), `last_used_at` |
| `src/models/user.rs` | yes | yes | `User` (never serializes `password_digest`), `NewUser` (email format, password 8 to 128 characters), `find`, `find_by_email`, `create`, `authenticate`, `update_password`; emails trimmed and lowercased |
| `src/models/auth_token.rs` | yes | | `issue`, `peek`, `consume` (valid 15 minutes, single use) |
| `src/models/api_key.rs` | yes | yes | `create` (returns the key once), `for_user`, `revoke`, `authenticate` |
| `src/auth.rs` | yes | | `CurrentUser`, `OptionalUser`, `sign_in`, `sign_out`, `origin` |
| `src/registrations.rs` | yes | | `GET/POST /signup`, `GET /account` (an example protected page) |
| `src/sessions.rs` | yes | | `GET/POST /login`, `POST /logout`, `GET/POST /magic_link`, `GET/POST /magic_link/{token}` |
| `src/passwords.rs` | yes | | `GET /passwords/new`, `POST /passwords`, `GET/POST /passwords/{token}` |
| `templates/auth/*.html` | yes | | The pages |
| `src/auth_api.rs` | yes | yes | `BearerUser`; `POST /api/auth/signup`, `POST /api/auth/token`, `GET /api/auth/me`, `GET/POST /api/auth/keys`, `DELETE /api/auth/keys/{id}` |

In an API-only app the output is:

```text
  create  migrations/0001_create_users.sql
  create  migrations/0002_create_api_keys.sql
  create  src/models/mod.rs
  create  src/models/user.rs
  create  src/models/api_key.rs
  create  src/auth_api.rs
  update  src/lib.rs

Next:
  ocre migrate
  ocre dev
  curl -X POST http://localhost:8787/api/auth/signup -H 'content-type: application/json' -d '{"email":"ada@example.com","password":"correct horse"}'
```

Running it a second time fails:

```text
error: this app already has a User model or a users table
hint: `ocre g auth` creates both and runs once per app; to start over, remove src/models/user.rs and the create_users migration
```

Run `ocre routes` to see every route the generator added.

## Sign-up, login and logout

The HTML flows are ordinary forms; the examples use curl with a cookie jar (`-c` saves cookies, `-b` sends them) to show what a browser gets.

A visitor who opens a protected page is sent to `/login`, and the page is remembered:

```sh
curl -si -c jar.txt -b jar.txt http://localhost:8787/account
```

```text
HTTP/1.1 303 See Other
Content-Length: 0
Location: /login
Set-Cookie: _ocre_session=OomyKZrKiIV5EQpPSEG9Ce4hmLjenLFK8MCStDD4c0RVxh4hgPGVu4hMn%2FKFAmNCCW0v0WmFNC3IfOX6ch2l1qvsfg+X%2FjJmaAexvIoNgANOYmXUhTSzyuvNoO2FZK+D0+8D2Q%3D%3D; HttpOnly; SameSite=Lax; Path=/
...
```

Signing up creates the user, signs them in and returns to that page:

```sh
curl -si -c jar.txt -b jar.txt http://localhost:8787/signup -d 'email=ada@example.com&password=correct horse'
```

```text
HTTP/1.1 303 See Other
Content-Length: 0
Location: /account
Set-Cookie: _ocre_session=HZlSU2bCVeNT7CWPgmbw2p6DaYDY9S%2FOB3S8Wu%2FiKTdXxippgjk2zgYIuqY6JvzqswPId3bRHmAF7SYVfWT0MZI0Hrv6legGNBfwImUsSW06alc2QQRbh5tC6livVjw%3D; HttpOnly; SameSite=Lax; Path=/
...
```

`GET /account` now shows the flash message and the user:

```text
<h1>Your account</h1>
<p class="notice">Welcome! Your account is ready.</p>
...
  <dt>Email</dt><dd>ada@example.com</dd>
  <dt>Member since</dt><dd>2026-09-29 04:32:55</dd>
...
```

Invalid input re-renders the form with status 422 and every message at once (emails are compared without case):

```sh
curl -s http://localhost:8787/signup -d 'email=ADA@example.com&password=short'
```

```text
...
  <ul class="errors">
    <li>Password is too short (minimum is 8 characters)</li><li>Email has already been taken</li>
  </ul>
...
```

Log out (`POST`, so other sites cannot log users out with a link), then log in:

```sh
curl -s -c jar.txt -b jar.txt -X POST http://localhost:8787/logout -o /dev/null -w '%{http_code} %{redirect_url}\n'
curl -s -c jar.txt -b jar.txt http://localhost:8787/login -d 'email=ada@example.com&password=wrong' -o /dev/null -w '%{http_code}\n'
curl -s -c jar.txt -b jar.txt http://localhost:8787/login -d 'email=Ada@Example.com&password=correct horse' -o /dev/null -w '%{http_code} %{redirect_url}\n'
```

```text
303 http://localhost:8787/
422
303 http://localhost:8787/
```

The failed login re-renders the form with "Invalid email or password." whether the email or the password was wrong. After a successful login the user goes back to the page that asked for it, or `/`.

## Magic links

`GET /magic_link` shows a form asking for an email; `POST /magic_link` emails a sign-in link valid 15 minutes. The answer is the same redirect to `/login` ("If an account exists for that email, a sign-in link is on its way.") whether or not the address has an account:

```sh
curl -s http://localhost:8787/magic_link -d 'email=ada@example.com' -o /dev/null -w '%{http_code} %{redirect_url}\n'
curl -s http://localhost:8787/magic_link -d 'email=nobody@example.com' -o /dev/null -w '%{http_code} %{redirect_url}\n'
```

```text
303 http://localhost:8787/login
303 http://localhost:8787/login
```

With `MAIL_ADAPTER=log` (what `ocre new` puts in `.dev.vars`) the email is printed in the `ocre dev` output:

```text
[ocre mail] not sent (MAIL_ADAPTER = "log")
From: shop <noreply@example.com>
To: ada@example.com
Subject: Your sign-in link

Open this link within 15 minutes to sign in:

http://localhost:8787/magic_link/e_ZL4u_uKZl0rYbpJe82lyqwQUPnFZTCOzUC59bGE2E

If you did not ask for it, ignore this email.

[ocre mail] HTML version:
<p><a href="http://localhost:8787/magic_link/e_ZL4u_uKZl0rYbpJe82lyqwQUPnFZTCOzUC59bGE2E">Sign in</a> (valid 15 minutes).</p><p>If you did not ask for it, ignore this email.</p>
[ocre mail] end
```

Opening the link (`GET`) shows a page with a "Sign in" button; the button `POST`s to the same URL, which uses the token up and signs in. Mail scanners that follow links therefore do not consume it. A second use fails:

```sh
curl -s -c jar.txt -b jar.txt -X POST http://localhost:8787/magic_link/e_ZL4u_uKZl0rYbpJe82lyqwQUPnFZTCOzUC59bGE2E -o /dev/null -w '%{http_code} %{redirect_url}\n'
curl -s -c jar.txt -b jar.txt -X POST http://localhost:8787/magic_link/e_ZL4u_uKZl0rYbpJe82lyqwQUPnFZTCOzUC59bGE2E -o /dev/null -w '%{http_code} %{redirect_url}\n'
```

```text
303 http://localhost:8787/
303 http://localhost:8787/magic_link
```

The second redirect carries the alert "That sign-in link is invalid or has expired." Requesting a new link cancels the previous one.

## Password reset

`GET /passwords/new` asks for an email; `POST /passwords` emails a reset link (same answer whether or not the account exists):

```sh
curl -s http://localhost:8787/passwords -d 'email=ada@example.com' -o /dev/null -w '%{http_code} %{redirect_url}\n'
```

```text
303 http://localhost:8787/login
```

```text
[ocre mail] not sent (MAIL_ADAPTER = "log")
From: shop <noreply@example.com>
To: ada@example.com
Subject: Reset your password

Open this link within 15 minutes to choose a new password:

http://localhost:8787/passwords/B8iQKwfm0L86-KOJuVtrep857CdZO0yfERAlZ3ntLFs
...
```

`GET /passwords/{token}` shows the form (an invalid or expired token redirects to `/passwords/new` with an alert). `POST /passwords/{token}` checks `password` and `password_confirmation`, then uses the token up, changes the password and redirects to `/login`:

```sh
T=B8iQKwfm0L86-KOJuVtrep857CdZO0yfERAlZ3ntLFs
curl -s http://localhost:8787/passwords/$T -d 'password=new password&password_confirmation=typo' -w '\n%{http_code}\n'
curl -s http://localhost:8787/passwords/$T -d 'password=new password 2&password_confirmation=new password 2' -o /dev/null -w '%{http_code} %{redirect_url}\n'
```

```text
...
  <ul class="errors">
    <li>Password confirmation doesn&#39;t match Password</li>
  </ul>
...
422
303 http://localhost:8787/login
```

A validation error does not use the token up, so the user can correct the form. A password change does not sign out other browsers: sessions live in cookies (see [What is not included](#what-is-not-included)).

## Protect HTML pages

Take one of the extractors from `src/auth.rs` as a handler argument:

| Extractor | Gives | Visitor without a session |
|---|---|---|
| `CurrentUser(user): CurrentUser` | `User` | Redirected to `/login` with the alert "Please log in to continue."; for `GET` requests the page is remembered and the user comes back to it after logging in |
| `OptionalUser(user): OptionalUser` | `Option<User>` | `None`; the page renders |

Both read `user_id` from the session and load the user with one D1 query. This module adds a dashboard for signed-in users and a greeting for everyone; register it with `mod dashboard;` under `// ocre:modules` and `.merge(dashboard::routes())` under `// ocre:routes` in `src/lib.rs`:

```rust,check
// src/dashboard.rs
use askama::Template;
use axum::{Router, extract::State, response::Html, routing::get};
use ocre::{Ctx, Flash, Result, render};

use crate::{
    auth::{CurrentUser, OptionalUser},
    models::{
        api_key::{self, ApiKey},
        user::User,
    },
};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/dashboard", get(show)).route("/hello", get(hello))
}

#[derive(Template)]
#[template(
    source = r#"{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
<h1>Dashboard of {{ user.email }}</h1>
<ul>{% for key in keys %}<li>{{ key.name }}</li>{% endfor %}</ul>
<form method="post" action="/logout"><button>Log out</button></form>"#,
    ext = "html"
)]
struct DashboardView {
    flash: Flash,
    user: User,
    keys: Vec<ApiKey>,
}

/// Visitors are redirected to /login, then back here after logging in.
async fn show(State(ctx): State<Ctx>, CurrentUser(user): CurrentUser, flash: Flash) -> Result<Html<String>> {
    let keys = api_key::for_user(&ctx, user.id).await?;
    render(&DashboardView { flash, user, keys })
}

/// Works for everyone; greets signed-in users by email.
async fn hello(OptionalUser(user): OptionalUser) -> String {
    match user {
        Some(user) => format!("Hello, {}", user.email),
        None => "Hello, visitor".to_owned(),
    }
}
```

Run against `ocre dev`, signed out then signed in:

```sh
curl -s -c jar.txt -b jar.txt http://localhost:8787/dashboard -o /dev/null -w '%{http_code} %{redirect_url}\n'
curl -s -c jar.txt -b jar.txt http://localhost:8787/login -d 'email=Ada@Example.com&password=correct horse' -o /dev/null -w '%{http_code} %{redirect_url}\n'
curl -s -c jar.txt -b jar.txt http://localhost:8787/dashboard
curl -s -b jar.txt http://localhost:8787/hello; echo
curl -s http://localhost:8787/hello; echo
```

```text
303 http://localhost:8787/login
303 http://localhost:8787/dashboard
<p class="notice">Signed in.</p>
<h1>Dashboard of ada@example.com</h1>
<ul></ul>
<form method="post" action="/logout"><button>Log out</button></form>
Hello, ada@example.com
Hello, visitor
```

To sign a user in from your own code (after an invitation, say), call `auth::sign_in(&session, &user)?`: it empties the session first, stores only the user id, and returns the path to redirect to. Sign out with `auth::sign_out(&session)?`. Never put more than the id in the session.

## JSON clients: JWTs and API keys

JSON clients cannot rely on the session cookie; they send `Authorization: Bearer <token>`, where the token is a JWT or an API key. These routes come from `src/auth_api.rs`:

| Route | Body | Answer |
|---|---|---|
| `POST /api/auth/signup` | `{"email", "password"}` | 201, the user |
| `POST /api/auth/token` | `{"email", "password"}` | `{"token", "token_type": "Bearer", "expires_in": 3600}` |
| `GET /api/auth/me` | | The user of the token |
| `GET /api/auth/keys` | | The user's API keys (never the secrets) |
| `POST /api/auth/keys` | `{"name"}` | 201, `{"key", "api_key"}`: `key` is shown this once |
| `DELETE /api/auth/keys/{id}` | | 204, or 404 when the user has no such key |

Sign up and get a JWT:

```sh
curl -s -X POST http://localhost:8787/api/auth/signup -H 'content-type: application/json' \
  -d '{"email":"grace@example.com","password":"correct horse"}'
curl -s -X POST http://localhost:8787/api/auth/token -H 'content-type: application/json' \
  -d '{"email":"grace@example.com","password":"correct horse"}'
```

```text
{"id":2,"email":"grace@example.com","created_at":"2026-09-29 04:35:02","updated_at":"2026-09-29 04:35:02"}
{"token":"eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIyIiwiaWF0IjoxNzkwNjU2NTAyLCJleHAiOjE3OTA2NjAxMDJ9.tucAZ1VrmMVeU7i0etJVIr568y13Iit2xJwJdsXs4Ag","token_type":"Bearer","expires_in":3600}
```

Wrong credentials, or a missing or invalid token, get a JSON 401 with `WWW-Authenticate: Bearer`:

```sh
curl -si -X POST http://localhost:8787/api/auth/token -H 'content-type: application/json' \
  -d '{"email":"grace@example.com","password":"nope"}'
```

```text
HTTP/1.1 401 Unauthorized
Transfer-Encoding: chunked
Content-Type: application/json
WWW-Authenticate: Bearer
...

{"error":{"message":"Unauthorized","status":401}}
```

Use the JWT, then create a long-lived API key with it:

```sh
TOKEN=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...
curl -s http://localhost:8787/api/auth/me -H "Authorization: Bearer $TOKEN"
curl -s -X POST http://localhost:8787/api/auth/keys -H "Authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' -d '{"name":"CI deploys"}'
curl -s http://localhost:8787/api/auth/keys -H "Authorization: Bearer $TOKEN"
```

```text
{"id":2,"email":"grace@example.com","created_at":"2026-09-29 04:35:02","updated_at":"2026-09-29 04:35:02"}
{"key":"5DNdPEYb0h2uCpRSUS4jFm-i05HI1DthdAgyL5tL_aY","api_key":{"id":1,"user_id":2,"name":"CI deploys","last_used_at":null,"created_at":"2026-09-29 04:35:11"}}
[{"id":1,"user_id":2,"name":"CI deploys","last_used_at":null,"created_at":"2026-09-29 04:35:11"}]
```

The key works like a JWT: `Authorization: Bearer 5DNd...`. It is used in the next sections as `$KEY`, and revoked at the end of [Records that belong to a user](#records-that-belong-to-a-user).

JWTs versus API keys:

| | JWT | API key |
|---|---|---|
| Lifetime | 1 hour (`TOKEN_TTL_SECONDS` in `src/auth_api.rs`) | Until revoked |
| Revocable | No, it expires | Yes, `DELETE /api/auth/keys/{id}` |
| Check cost | One HMAC-SHA256, no D1 read to verify (plus the user lookup) | One D1 read by digest, plus a `last_used_at` write at most once an hour |
| Use for | Apps that log in with a password | Scripts, CI, integrations |

Bearer requests without an `Origin` or `Sec-Fetch-Site` header (curl, servers, mobile apps) pass the CSRF check; a separate browser frontend on another origin needs `ALLOWED_ORIGINS` (see [Sessions, flash and security](security.md#allowed_origins-another-site-calling-the-app)).

## Protect JSON endpoints

Take `BearerUser(user): BearerUser` from `src/auth_api.rs`. It accepts a JWT (anything with dots) or an API key, loads the user, and otherwise answers the JSON 401 above; a deleted user's tokens stop working. Put it before `Json(..)` in the arguments (axum runs the body extractor last).

This endpoint returns one of the caller's API keys. The `user_id` condition in the query is the ownership check: another user's key is "not found", exactly like a missing one, so the answer reveals nothing. Register it like the dashboard module:

```rust,check
// src/my_keys_api.rs
use axum::{
    Router,
    extract::{Path, State},
    routing::get,
};
use ocre::{ApiResult, Ctx, Json, OptionExt, Result, params};

use crate::{auth_api::BearerUser, models::api_key::ApiKey};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/api/my/keys/{id}", get(show))
}

/// One of the user's API keys. The `user_id` condition is the ownership
/// check: another user's key is "not found", exactly like a missing one.
pub async fn find_owned(ctx: &Ctx, user_id: i64, id: i64) -> Result<Option<ApiKey>> {
    ctx.db()?
        .first(
            "SELECT id, user_id, name, last_used_at, created_at FROM api_keys WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .await
}

/// 401 JSON without a valid `Authorization: Bearer <JWT or API key>`.
async fn show(State(ctx): State<Ctx>, BearerUser(user): BearerUser, Path(id): Path<i64>) -> ApiResult<Json<ApiKey>> {
    Ok(Json(find_owned(&ctx, user.id, id).await?.or_404()?))
}
```

With Grace's API key, then with a JWT of Ada (user 1, from `POST /api/auth/token` with her email and password):

```sh
KEY=5DNdPEYb0h2uCpRSUS4jFm-i05HI1DthdAgyL5tL_aY
ADA=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIiwiaWF0IjoxNzkwNjU2NTE3LCJleHAiOjE3OTA2NjAxMTd9.j6v-x42xurVULPJno1bh4lxP038jiRKVAFqac45QZoY
```

```sh
curl -s http://localhost:8787/api/my/keys/1 -H "Authorization: Bearer $KEY"
curl -s http://localhost:8787/api/my/keys/1 -H "Authorization: Bearer $ADA" -w ' %{http_code}\n'
```

```text
{"id":1,"user_id":2,"name":"CI deploys","last_used_at":"2026-09-29 04:35:17","created_at":"2026-09-29 04:35:11"}
{"error":{"message":"Not found","status":404}} 404
```

`last_used_at` was set by the first request made with the key.

## Records that belong to a user

Give a table a `user_id` column with `references`, then filter every query by the signed-in user's id.

```sh
ocre g model Note title:string user:references
```

```text
  create  migrations/0005_create_notes.sql
  create  src/models/note.rs
  update  src/models/user.rs
  update  src/models/mod.rs

Next:
  ocre migrate
  cargo check --target wasm32-unknown-unknown
```

The migration has `user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE` (a user's notes are deleted with the user) and an index on `user_id`. `src/models/note.rs` gets `note.user(&ctx)`, and `src/models/user.rs` gets the list query, already filtered by owner:

```rust
impl User {
    // ocre:associations
    /// Notes of this user, newest first.
    pub async fn notes(&self, ctx: &Ctx, page: ocre::Page) -> Result<Vec<crate::models::note::Note>> {
        ctx.db()?
            .all(
                "SELECT * FROM notes WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3",
                params![self.id, page.limit, page.offset],
            )
            .await
    }
}
```

A JSON API over the notes then takes the owner from the token, never from the request body, and answers 404 for other users' notes:

```rust
// src/notes_api.rs
use axum::{
    Router,
    extract::{Path, State},
    routing::get,
};
use ocre::{ApiResult, Created, Ctx, Error, Json, Page};
use serde::Deserialize;

use crate::{
    auth_api::BearerUser,
    models::note::{self, NewNote, Note},
};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/api/notes", get(index).post(create)).route("/api/notes/{id}", get(show))
}

#[derive(Deserialize)]
struct NoteInput {
    title: String,
}

async fn index(State(ctx): State<Ctx>, BearerUser(user): BearerUser, page: Page) -> ApiResult<Json<Vec<Note>>> {
    Ok(Json(user.notes(&ctx, page).await?))
}

async fn create(State(ctx): State<Ctx>, BearerUser(user): BearerUser, Json(input): Json<NoteInput>) -> ApiResult<Created<Note>> {
    // The owner comes from the token, never from the request body.
    Ok(Created(note::create(&ctx, NewNote { title: input.title, user_id: user.id }).await?))
}

async fn show(State(ctx): State<Ctx>, BearerUser(user): BearerUser, Path(id): Path<i64>) -> ApiResult<Json<Note>> {
    match note::find(&ctx, id).await? {
        Some(note) if note.user_id == user.id => Ok(Json(note)),
        _ => Err(Error::NotFound.into()),
    }
}
```

Grace creates a note; Ada cannot see it:

```sh
curl -s -X POST http://localhost:8787/api/notes -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' -d '{"title":"Buy flour"}'
curl -s http://localhost:8787/api/notes/1 -H "Authorization: Bearer $ADA" -w ' %{http_code}\n'
curl -s http://localhost:8787/api/notes -H "Authorization: Bearer $ADA"; echo
```

```text
{"id":1,"title":"Buy flour","user_id":2,"created_at":"2026-09-29 04:35:17","updated_at":"2026-09-29 04:35:17"}
{"error":{"message":"Not found","status":404}} 404
[]
```

Finally, Grace revokes her API key; it stops working at once:

```sh
curl -s -X DELETE http://localhost:8787/api/auth/keys/1 -H "Authorization: Bearer $KEY" -w '%{http_code}\n'
curl -s http://localhost:8787/api/auth/me -H "Authorization: Bearer $KEY" -w ' %{http_code}\n'
```

```text
204
{"error":{"message":"Unauthorized","status":401}} 401
```

Rules for owned records:

- Filter in SQL (`WHERE id = ?1 AND user_id = ?2`) for updates and deletes, so one statement both checks and acts; `api_key::revoke` in `src/models/api_key.rs` does this.
- Answer `Error::NotFound` for other users' records, so ids of other users' data are not confirmed. Use `Error::Forbidden` (403) when the user may know the record exists but may not change it (a shared document that only its owner edits).
- The HTML scaffold pages of `ocre g scaffold` do not check ownership; add `CurrentUser` and the `user_id` filter to each handler that should be private.

## Add a column to users

Add a migration, apply it, then update the model by hand (the generator does not edit `user.rs`):

```sh
ocre g migration add_name_to_users name:string?
ocre migrate
```

```text
  create  migrations/0006_add_name_to_users.sql

Next:
  ocre migrate
  update the model in src/models/ to match the new columns
```

```sql
-- Migration: add_name_to_users
-- Applied once, in file-name order. Never edit a migration after it has been applied.
ALTER TABLE users ADD COLUMN name TEXT;
```

In `src/models/user.rs`: add `pub name: Option<String>,` to `User` (it is serialized in JSON answers; keep `#[serde(skip_serializing)]` for anything secret), `pub name: String,` to `NewUser` (missing in a form or JSON body means empty, thanks to `#[serde(default)]`), a rule in `NewUser::validate` (`v.max_length("name", &self.name, 100);`), and the column in the `INSERT` of `create`:

```rust
    let name = Some(new.name.trim()).filter(|name| !name.is_empty());
    db.first(
        "INSERT INTO users (email, password_digest, name) VALUES (?1, ?2, ?3) RETURNING *",
        params![email, password_digest, name],
    )
```

For the HTML sign-up, add `<label>Name <input name="name"></label>` to `templates/auth/signup.html`. The JSON sign-up takes it right away:

```sh
curl -s -X POST http://localhost:8787/api/auth/signup -H 'content-type: application/json' \
  -d '{"email":"linus@example.com","password":"correct horse","name":"Linus"}'
curl -s -X POST http://localhost:8787/api/auth/signup -H 'content-type: application/json' \
  -d '{"email":"bad","password":"short"}'
```

```text
{"id":3,"email":"linus@example.com","name":"Linus","created_at":"2026-09-29 04:42:46","updated_at":"2026-09-29 04:42:46"}
{"error":{"fields":{"email":["is invalid"],"password":["is too short (minimum is 8 characters)"]},"message":"Validation failed","status":422}}
```

## Framework primitives

The generated code is built on these `ocre` items; use them for your own flows (invitations, email confirmation, one-time codes):

| Item | What it does | Cost |
|---|---|---|
| `ocre::password::hash(&password).await?` | PBKDF2-HMAC-SHA256 digest, `pbkdf2_sha256$100000$<salt>$<hash>` (16-byte random salt) | About 5.5 ms of CPU (WebCrypto, outside WebAssembly): one per request at most |
| `ocre::password::verify(&password, &digest).await?` | `bool`, constant-time comparison | Same as `hash` |
| `ocre::password::iterations(&digest)` | The iteration count stored in a digest | None |
| `ocre::token::generate()` | 256 random bits, URL-safe base64 (43 characters) | None |
| `ocre::token::digest(&token)` | SHA-256 of a token, to store and look up instead of the token | None |
| `ocre::token::constant_time_eq(a, b)` | Compares secrets without timing leaks | None |
| `ocre::jwt::encode(&ctx, &Claims::new(user.id.to_string(), ttl))?` | HS256 JWT with `sub`, `iat`, `exp`; key derived from `SECRET_KEY_BASE` | One HMAC |
| `ocre::jwt::decode(&ctx, &token)?` | `Claims`, or `Error::Unauthorized` when expired, badly signed, or not HS256 (including `alg: none`) | One HMAC |
| `ocre::now()` | Unix seconds; `SystemTime::now()` panics on `wasm32-unknown-unknown` | None |
| `Error::Unauthorized` | 401; JSON answers add `WWW-Authenticate: Bearer` | |
| `Error::Forbidden` | 403, for "signed in but not allowed" | |

Store only `token::digest(&token)` for secrets you email or show once, and look rows up by digest, as `src/models/auth_token.rs` and `src/models/api_key.rs` do. The full signatures are in the [API index](../api-index.md) and the rustdoc of [`ocre::password`](/api/ocre/password/index.html), [`ocre::token`](/api/ocre/token/index.html) and [`ocre::jwt`](/api/ocre/jwt/index.html).

## Before going public

- **Mail.** Magic links and password resets call `ocre::mail::send`. Without `MAIL_ADAPTER` in production, `POST /magic_link` and `POST /passwords` answer 500 and the log says `cannot send email: MAIL_ADAPTER is not set. Fix: ...`. For sign-up and reset mail to any address on the free plan, use Resend (100 emails a day, 3,000 a month, September 2026); see [Email](email.md#choose-an-adapter).
- **Rate limiting.** Nothing limits attempts: login, sign-up, magic-link, reset and token routes accept unlimited requests, and each password check costs about 5.5 ms of CPU. Add [Cloudflare rate limiting rules](https://developers.cloudflare.com/waf/rate-limiting-rules/) for `/login`, `/signup`, `/magic_link`, `/passwords` and `/api/auth/*` before going public. These rules belong to a zone, so they need the app on a custom domain or route: requests to the `*.workers.dev` host are not covered ([workers.dev](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/) recommends a custom domain for production). Inside the Worker, Cloudflare's [Rate Limiting binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) is another option; Ocre does not wrap it.
- **Secret.** `ocre deploy` creates `SECRET_KEY_BASE` on the first deploy; never commit `.dev.vars`.

## Security choices

In short (the reasoning is in [Security model](../explanations/security-model.md)):

- **Passwords**: PBKDF2-HMAC-SHA256 with 100,000 iterations through WebCrypto, the most Workers accept (OWASP recommends 600,000; bcrypt or argon2 in WebAssembly would not fit in 10 ms of CPU). A login with an unknown email hashes too, so response times do not reveal accounts.
- **Sessions**: the encrypted cookie holds only `user_id`; login empties the session first, logout clears it. `CurrentUser` only returns to local paths (no open redirect).
- **Emailed tokens**: 256 random bits, stored as SHA-256 digests, valid 15 minutes, used up by one `DELETE ... RETURNING` (single use even under concurrent requests); a new request cancels the previous link; links open a page whose button `POST`s. Links use the request's origin.
- **JWTs**: HS256 only, key derived from `SECRET_KEY_BASE` with a fixed label (so it differs from the cookie key); no separate `JWT_SECRET`. Rotating `SECRET_KEY_BASE` signs everyone out of sessions and tokens at once.
- **API keys**: 256 random bits shown once, stored as SHA-256 digests, revocable; `last_used_at` written at most once an hour to save D1 writes.

## What is not included

- Rate limiting (see [Before going public](#before-going-public)).
- Email confirmation of new accounts.
- "Sign out everywhere": sessions last until logout, the end of the browser session, or a `SECRET_KEY_BASE` change; a password reset does not end other sessions.
- Roles and permissions: `Error::Forbidden` is there for your own checks.
- Revoking a JWT before it expires (use API keys for long-lived access).

## See also

- [`ocre g auth`](../reference/generators.md#ocre-g-auth) in the generators reference.
- [Sessions, flash and security](security.md): sessions, CSRF, `ALLOWED_ORIGINS`, headers.
- [Email](email.md): mail adapters for magic links and password resets.
- [JSON APIs and GraphQL](json-apis.md): `ApiResult`, errors, pagination.
- [Models and migrations](models.md): `references`, migrations, associations.
- [Security model](../explanations/security-model.md).
