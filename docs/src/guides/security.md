# Sessions, flash and security

This guide shows how an Ocre app keeps per-visitor state in an encrypted session cookie, shows one-time flash messages, and what `ocre::serve` does for every request to protect it: host checks, cross-site request (CSRF) checks, CORS, security headers and cookie flags. It then covers the helpers an app calls itself (rate limiting, safe queries and parameters, files, user HTML) and ends with what is not included, so you know what to add yourself.

## Before you start

- An app created with [`ocre new`](../reference/cli.md#ocre-new). Everything on this page is built into the `ocre` crate and applied by `ocre::serve` in `src/lib.rs`; no generator is needed.
- The `SECRET_KEY_BASE` secret (see [Configuration](../reference/configuration.md#secret_key_base)). `ocre new` writes a random one to `.dev.vars` for `ocre dev`; `ocre deploy` uploads a new one to the Worker the first time (see [Keys and rotation](#keys-and-rotation)).
- The outputs below come from an app created with `ocre new shop --starter blog`, served by `ocre dev` on the default port 8787.

## What every request goes through

`ocre::serve(routes(), req, env)` wraps the app's router with five layers, outermost first:

| Order | Layer | Effect |
|---|---|---|
| 1 | Security headers | Adds `nosniff`, `SAMEORIGIN` framing, a referrer policy and more to every response; HSTS on HTTPS |
| 2 | Host authorization | Only when `ALLOWED_HOSTS` is set: requests for other host names get 403 |
| 3 | CORS | Only when `ALLOWED_ORIGINS` is set: answers preflights and adds `Access-Control-*` headers for those origins |
| 4 | Cross-origin protection (CSRF) | Refuses unsafe requests a browser sends from another site, with 403 |
| 5 | Session | Decrypts the session cookie on first use and sends `Set-Cookie` when the handler changed it |

None of these layers makes a D1, KV or network call: they cost a little CPU and no free-plan quota. Generated full-stack apps add a Content-Security-Policy and a Permissions-Policy layer in `src/lib.rs` (see [Content-Security-Policy and Permissions-Policy](#content-security-policy-and-permissions-policy)).

## Sessions

A handler takes `session: ocre::Session` as an argument (an axum extractor) and reads or writes values by key. Values are anything `serde` can serialize; they are stored as JSON.

| Method | Returns | Effect |
|---|---|---|
| `session.get::<T>("key")?` | `Option<T>` | The value, or `None` when missing or not of type `T` |
| `session.insert("key", value)?` | `()` | Stores `value` (any `Serialize`) |
| `session.remove("key")?` | `bool` | Removes the key; `true` when it was there |
| `session.clear()?` | `()` | Empties the session (sign out); pending flash messages are kept |
| `session.flash("notice", "...")?` | `()` | A message for the next request (see [Flash messages](#flash-messages)) |
| `session.flashes()?` | `Flash` | The flash messages that arrived with this request |

Every method returns `ocre::Result`, so use `?`. Changes are sent back as one `Set-Cookie` header when the handler returns. A request that does not change the session sends no cookie; taking pending flash messages out counts as a change.

The session is a cookie, like Rails' default cookie store: no D1 rows, no KV operations, nothing on the server.

- **Encrypted and authenticated.** The cookie value is JSON encrypted with AES-256-GCM, with a key derived from `SECRET_KEY_BASE`. Clients can neither read nor change it. A cookie that does not decrypt (tampered, truncated, or encrypted with a key that is neither `SECRET_KEY_BASE` nor listed in `SECRET_KEY_BASE_PREVIOUS`) is ignored: the request starts with an empty session, no error.
- **4 KB at most.** Browsers drop cookies over 4096 bytes (name, value and attributes). When a change would make the cookie larger, the response is a 500 and the log says `the session cookie would be <n> bytes; browsers drop cookies over 4096. Fix: store ids in the session, not records`. Store ids and short strings, never records, and never secrets such as tokens or passwords.
- **Cookie flags.** The cookie is `_ocre_session`, `Path=/`, `HttpOnly` (JavaScript cannot read it), `SameSite=Lax`, and `Secure` when the request came over HTTPS (always on `*.workers.dev`; not on `http://localhost`). By default it has no `Max-Age`: it lasts until the browser session ends or the app clears it; `session.remember_for(seconds)` makes it persistent (see [Expiry and remember me](#expiry-and-remember-me)).
- **Clearing.** When the session becomes empty, the cookie is deleted (`Max-Age=0`).

### Store a value in the session

This module stores a visitor's theme in the session and confirms the change with a flash message. It needs nothing but the `ocre new` app: add `mod preferences;` under `// ocre:modules` and `.merge(preferences::routes())` under `// ocre:routes` in `src/lib.rs`.

```rust,check
// src/preferences.rs
use askama::Template;
use axum::{
    Form, Router,
    response::{Html, Redirect},
    routing::{get, post},
};
use ocre::{Ctx, Error, Flash, Result, Session, render};
use serde::Deserialize;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/preferences", get(show).post(update)).route("/preferences/reset", post(reset))
}

/// Session key; the value is a short string, never a record.
const THEME: &str = "theme";

#[derive(Template)]
#[template(
    source = r#"{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
<p>Theme: {{ theme }}</p>
<form method="post" action="/preferences">
  <select name="theme"><option>light</option><option>dark</option></select>
  <button>Save</button>
</form>"#,
    ext = "html"
)]
struct PreferencesView {
    flash: Flash,
    theme: String,
}

#[derive(Deserialize)]
struct PreferencesForm {
    theme: String,
}

async fn show(session: Session, flash: Flash) -> Result<Html<String>> {
    let theme = session.get::<String>(THEME)?.unwrap_or_else(|| "light".to_owned());
    render(&PreferencesView { flash, theme })
}

async fn update(session: Session, Form(form): Form<PreferencesForm>) -> Result<Redirect> {
    if form.theme != "light" && form.theme != "dark" {
        return Err(Error::bad_request("theme must be light or dark"));
    }
    session.insert(THEME, &form.theme)?;
    session.flash("notice", "Preferences saved.")?;
    Ok(Redirect::to("/preferences"))
}

async fn reset(session: Session) -> Result<Redirect> {
    if session.remove(THEME)? {
        session.flash("notice", "Preferences reset.")?;
    }
    Ok(Redirect::to("/preferences"))
}
```

In a real page, put the markup in `templates/preferences/show.html` with `{% extends "layout.html" %}` and use `#[template(path = "preferences/show.html")]`; the inline `source` keeps the example in one file.

Try it with `ocre dev` running and a cookie jar (`-c` saves cookies, `-b` sends them):

```sh
curl -s -c jar.txt -b jar.txt -X POST http://localhost:8787/preferences -d theme=dark -o /dev/null -w '%{http_code} %{redirect_url}\n'
curl -s -c jar.txt -b jar.txt -D - http://localhost:8787/preferences
curl -s -c jar.txt -b jar.txt http://localhost:8787/preferences
```

```text
303 http://localhost:8787/preferences
HTTP/1.1 200 OK
Transfer-Encoding: chunked
Content-Type: text/html; charset=utf-8
Set-Cookie: _ocre_session=1ik14hJ5am+ra2dQbLBTz921QqhYB+TbkHC+SAM1DpFW8Tv9OVP7cOV74D8%3D; HttpOnly; SameSite=Lax; Path=/
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0

<p class="notice">Preferences saved.</p>
<p>Theme: dark</p>
...

<p>Theme: dark</p>
...
```

The first page shows the flash message and sends a new cookie (the message was taken out of the session); the second shows the theme without it. A forged cookie is simply ignored:

```sh
curl -s -H 'Cookie: _ocre_session=abc' http://localhost:8787/preferences
```

```text

<p>Theme: light</p>
...
```

And the 400 from `Error::bad_request` is rendered as an HTML error page:

```sh
curl -s -b jar.txt -X POST http://localhost:8787/preferences -d theme=pink
```

```text
<h1>400</h1><p>theme must be light or dark</p>
```

## Flash messages

A flash message is shown once, on the next page, usually after a redirect ("Post was successfully created."). Set it with `session.flash(kind, message)?` before returning a `Redirect`; the next handler takes `flash: ocre::Flash` and passes it to its template. Taking `Flash` removes the messages from the session, so a reload does not show them again.

| `Flash` method | Returns |
|---|---|
| `flash.notice()` | `Option<&str>`: the `"notice"` message (success) |
| `flash.alert()` | `Option<&str>`: the `"alert"` message (failure) |
| `flash.get("kind")` | `Option<&str>`: any other kind |
| `flash.iter()` | `(kind, message)` pairs |
| `flash.is_empty()` | `bool` |

Kinds are free-form; generated code uses `notice` and `alert`, and generated templates show them like this:

```html
{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
{% if let Some(alert) = flash.alert() %}<p class="alert">{{ alert }}</p>{% endif %}
```

Setting a second message of the same kind in one request replaces the first. `session.clear()` keeps pending flash messages, so "Signed out." survives the logout that empties the session. Flash messages live in the session cookie, so they count toward its 4 KB: keep them to a sentence.

For a message on the page being rendered (Rails' `flash.now`), add it to the `Flash` the view gets, without touching the session: `render(&EditView { flash: flash.now("alert", "Check the highlighted fields."), .. })`. To carry the messages one more request (Rails' `flash.keep`), for example when a page redirects again before showing them, call `session.keep_flash(&flash)?`.

## Cookies

The session is the place for per-visitor state. For cookies that outlive it or that other parts of the site read, take `ocre::Cookies` in the handler (Rails' `cookies`, `cookies.signed` and `cookies.encrypted`):

```rust,ignore
use std::time::Duration;
use ocre::{Cookies, Result};

async fn preferences(cookies: Cookies) -> Result<String> {
    cookies.set("theme", "dark", Some(Duration::from_secs(365 * 86_400)))?;       // readable and changeable by the browser
    cookies.set_signed("seen_banner", "1", None)?;                                 // readable, tamper-proof (HMAC)
    cookies.set_encrypted("remember_token", "a1b2c3", Some(Duration::from_secs(30 * 86_400)))?; // hidden and tamper-proof
    Ok(format!("{:?} {:?}", cookies.get("theme"), cookies.signed("seen_banner")?))
}
```

`get`, `signed` and `encrypted` read the request's cookies (`None` when absent, or changed by the client for the last two); `remove(name)` deletes one. Cookies set by a handler go out with the response, `HttpOnly`, `SameSite=Lax`, `Path=/` and `Secure` over HTTPS. Signed and encrypted cookies use keys derived from `SECRET_KEY_BASE`, and values made with a key of `SECRET_KEY_BASE_PREVIOUS` still read. The session cookie's name (`_ocre_session`) is refused.

## Keys and rotation

`SECRET_KEY_BASE` must be set and at least 64 characters long. `ocre secret` prints a new random value (128 hex characters):

```sh
ocre secret
```

Where it comes from:

| Environment | Source |
|---|---|
| `ocre dev` | `.dev.vars` (git-ignored), written by `ocre new` |
| Production | A Worker secret. `ocre deploy` checks `cf workers secrets list` and, when the Worker has no `SECRET_KEY_BASE`, uploads a new random one with the deploy; it never replaces an existing one |

Two keys are derived from it: the session cookie key, and (with a fixed label) the HS256 key of `ocre::jwt` (see [Authentication](authentication.md#json-clients-jwts-and-api-keys)). When the secret is missing or shorter than 64 characters, requests that write the session, or read a session cookie they received, answer 500, and the log names the fix: ``the SECRET_KEY_BASE secret is not set. Fix: run `ocre secret`, put the value in .dev.vars as SECRET_KEY_BASE=... for `ocre dev` (`ocre new` does this), and deploy with `ocre deploy`, which uploads it``.

**Replacing the secret alone signs everyone out.** Existing cookies no longer decrypt, so every visitor starts with an empty session, and every JWT fails to verify. This is the way to invalidate all cookie sessions at once, for example after a leak: put a new value from `ocre secret` in `.prod.vars` (git-ignored) as `SECRET_KEY_BASE=...`, then

```sh
ocre secrets push SECRET_KEY_BASE --file .prod.vars
```

Changing the value in `.dev.vars` and restarting `ocre dev` shows the effect: a browser that was signed in is redirected to `/login` by the next `CurrentUser` page, and an old JWT gets `401 {"error":{"message":"Unauthorized","status":401}}`. For a planned rotation that keeps everyone signed in, see [Rotating SECRET_KEY_BASE without signing everyone out](#rotating-secret_key_base-without-signing-everyone-out).

## Cross-site request forgery (CSRF)

Forms need no token. Instead, Ocre checks where each request comes from, the check Go 1.25 ships as `http.CrossOriginProtection`:

1. `GET`, `HEAD` and `OPTIONS` requests pass (they must not change data), except WebSocket handshakes, which are checked like forms because browsers send cookies with them.
2. A request whose `Origin` is listed in `ALLOWED_ORIGINS` passes.
3. When the browser sent `Sec-Fetch-Site` (all current browsers do), `same-origin` and `none` (typed in the address bar, bookmarks) pass; anything else (`cross-site`, `same-site`) is refused.
4. Older browsers that send only `Origin`: it must match the `Host` header, else the request is refused.
5. A request with neither header passes: it does not come from a web page, so it cannot carry a victim's cookies by accident (curl, server-to-server calls, mobile apps).

A refused request never reaches the handler. It gets a plain-text 403:

```sh
curl -si -X POST http://localhost:8787/preferences -H 'Sec-Fetch-Site: cross-site' -d theme=dark
```

```text
HTTP/1.1 403 Forbidden
Transfer-Encoding: chunked
Content-Type: text/plain; charset=utf-8
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0

Forbidden: cross-site request. Add the origin to ALLOWED_ORIGINS to allow it.
```

With only an `Origin` that does not match the host (older browsers):

```sh
curl -si -X POST http://localhost:8787/preferences -H 'Origin: https://evil.example' -d theme=dark
```

```text
HTTP/1.1 403 Forbidden
...
Forbidden: cross-origin request. Add the origin to ALLOWED_ORIGINS to allow it.
```

The same form posted from the app's own pages carries `Sec-Fetch-Site: same-origin` and goes through:

```sh
curl -si -X POST http://localhost:8787/preferences -H 'Sec-Fetch-Site: same-origin' -d theme=dark
```

```text
HTTP/1.1 303 See Other
Content-Length: 0
Location: /preferences
Set-Cookie: _ocre_session=w+0WnN%2FE6WOBxLRb+F69geJp1HvSBUuO9%2FQlgsaoC1cNBkPpegmJbFic45skI6OmixjEaRl52s1TSas1fRpkqRqL0k8otB9akF60KBkTSKGQ+7DTYQ%3D%3D; HttpOnly; SameSite=Lax; Path=/
...
```

Consequences for app code:

- Change data only in `POST`, `PUT`, `PATCH` and `DELETE` handlers. A `GET` handler that changes data is not protected. Generated scaffolds follow this (`POST /posts/{id}/delete`, `POST /logout`).
- htmx requests (`hx-post`, `hx-delete`...) and your own `fetch()` calls from the app's pages are same-origin and pass. There is no token to put in a `<meta>` tag or send in a header (Rails' `csrf_meta_tags`): the browser's `Sec-Fetch-Site` header does that job.
- The session cookie is also `SameSite=Lax`: browsers do not send it with cross-site `POST`s at all, a second layer for the same attack.
- A refused request never reaches a handler, so nothing needs undoing when the check fails: no session is read, and a "remember me" cookie cannot sign it in.

## ALLOWED_ORIGINS: another site calling the app

When a separate frontend (another domain, a local dev server on another port) must call the app from the browser, list its origins in the `ALLOWED_ORIGINS` Worker variable, comma-separated, in `worker.env` of `cloudflare.config.ts` (or in `.dev.vars` for `ocre dev`):

```ts
// cloudflare.config.ts, in worker.env
ALLOWED_ORIGINS: bindings.text("https://app.example.com, https://admin.example.com"),
```

Spaces and a trailing `/` are ignored; invalid entries are skipped. The listed origins get two things:

- **CORS**: preflights are answered, with credentials allowed (the browser may send cookies), methods `GET, POST, PUT, PATCH, DELETE`, request headers `Content-Type`, `Authorization`, `Accept`.
- **CSRF trust**: their unsafe requests pass the cross-origin check.

With `ALLOWED_ORIGINS=https://app.example.com` in `.dev.vars`, a preflight from that origin:

```sh
curl -si -X OPTIONS http://localhost:8787/api/notes \
  -H 'Origin: https://app.example.com' \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: authorization, content-type'
```

```text
HTTP/1.1 200 OK
Content-Length: 0
Access-Control-Allow-Origin: https://app.example.com
Allow: GET,HEAD,POST
Vary: origin
access-control-allow-credentials: true
access-control-allow-headers: content-type,authorization,accept
access-control-allow-methods: GET,POST,PUT,PATCH,DELETE
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0
```

A cross-site `POST` from `https://app.example.com` now gets `303 See Other` with `Access-Control-Allow-Origin: https://app.example.com`; the same request from any other origin still gets 403. When the variable is unset or empty, no CORS layer is added and only same-origin browser requests may change data. List only origins you control: a listed origin can act with the visitor's cookies.

## Security headers

Every response gets Rails' default headers. Captured from `GET /up`:

```sh
curl -sI http://localhost:8787/up
```

```text
HTTP/1.1 200 OK
Content-Length: 2
Content-Type: text/plain; charset=utf-8
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0
```

| Header | Value | Effect |
|---|---|---|
| `X-Content-Type-Options` | `nosniff` | Browsers trust `Content-Type` instead of guessing |
| `X-Frame-Options` | `SAMEORIGIN` | Other sites cannot put the app in a frame (clickjacking) |
| `Referrer-Policy` | `strict-origin-when-cross-origin` | Other sites see only the origin, never paths with tokens |
| `X-XSS-Protection` | `0` | Turns off the obsolete, exploitable XSS auditor of old browsers |
| `X-Permitted-Cross-Domain-Policies` | `none` | No Flash/PDF cross-domain policy files |
| `Strict-Transport-Security` | `max-age=63072000` | HTTPS requests only (two years); `ocre dev` over HTTP does not get it |

A handler that sets one of these headers keeps its own value. The same way, a handler can add headers Ocre does not send, such as a Content-Security-Policy:

```rust,check
// src/report.rs
use axum::{
    Router,
    http::header,
    response::{Html, IntoResponse},
    routing::get,
};
use ocre::Ctx;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/report", get(report))
}

async fn report() -> impl IntoResponse {
    (
        [
            // Not sent by Ocre: added here.
            (header::CONTENT_SECURITY_POLICY, "default-src 'self'"),
            // Sent by Ocre as strict-origin-when-cross-origin: this value wins.
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Html("<p>Quarterly report</p>"),
    )
}
```

Static files in `public/` are served by Cloudflare before the Worker runs, so they do not get these headers; add them with a [`_headers` file](https://developers.cloudflare.com/workers/static-assets/headers/) if needed.

## Escaping in templates

askama escapes every `{{ value }}` in `.html` templates. A post titled `<script>alert(1)</script>` is shown as text:

```text
<tr><td>&#60;script&#62;alert(1)&#60;/script&#62;</td><td>Hi</td>...
```

Rules:

- Never apply `|safe` to user input; it turns escaping off.
- `.txt` templates (plain-text email bodies) are not escaped, which is correct for plain text; do not reuse them for HTML.
- HTML built with `format!` is not escaped. Build HTML in templates, or escape values yourself.
- Values inside `<script>` or event attributes need JSON encoding, not HTML escaping; avoid putting user input there.

## SQL injection

D1 queries take `?1, ?2` placeholders and a `params![...]` list; the values never become SQL text, so they cannot inject SQL:

```rust
ctx.db()?.first("SELECT * FROM users WHERE email = ?1", params![email]).await
```

The `Query` builder (`post::query().eq("published", true)...`) binds every value the same way, and takes column names, joins, `where_sql` fragments and sort terms as `&'static str`: they can only come from your code, never from request text. `is_in` with an empty list matches no row instead of writing invalid or unbounded SQL, and `contains`, `starts_with` and `ends_with` escape `%` and `_` in the searched text.

Never build SQL with `format!` from user input. When a column or sort order comes from the request, `match` it onto a fixed column name:

```rust,check
// src/post_search.rs
use axum::{
    Json, Router,
    extract::{Query as Params, State},
    routing::get,
};
use ocre::{Ctx, Direction, Result};
use serde::Deserialize;

use crate::models::post::{self, Post};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/posts/search", get(search))
}

/// The only two parameters read from the query string; others are ignored.
#[derive(Deserialize)]
struct SearchParams {
    #[serde(default)]
    q: String,
    sort: Option<String>,
}

async fn search(State(ctx): State<Ctx>, Params(params): Params<SearchParams>) -> Result<Json<Vec<Post>>> {
    // The column comes from this list, never from the request text.
    let (column, direction) = match params.sort.as_deref() {
        Some("title") => ("title", Direction::Asc),
        _ => ("created_at", Direction::Desc),
    };
    let posts = post::query()
        .eq("published", true)
        .contains("title", &params.q)
        .order_by(column, direction)
        .limit(20)
        .all(&ctx.db()?)
        .await?;
    Ok(Json(posts))
}
```

`GET /posts/search?q=rust&sort=title` runs `SELECT * FROM posts WHERE published = ?1 AND title LIKE ?2 ESCAPE '\' ORDER BY title ASC LIMIT ?3`; `sort=id;DROP TABLE posts` sorts by `created_at`.

## Only the fields you declare (strong parameters)

Handlers read forms and JSON bodies into a struct (`Form<PostForm>`, `Json<NewPost>`, `Query<SearchParams>`). serde fills only the fields the struct declares and ignores the others, so the struct is the allowlist that Rails' `params.require(...).permit(...)` builds: a request that adds `user_id=1` or `"admin": true` changes nothing unless the struct has that field. Keep fields a visitor must not choose out of the form structs, and set them in the handler instead, like the owner from `CurrentUser` (see [Records that belong to a user](authentication.md#records-that-belong-to-a-user)). Add `#[serde(deny_unknown_fields)]` to a struct to reject such requests instead of ignoring the extra fields.

Types do the rest of Rails' parameter checks: a missing field, or text where the struct wants a number, a `bool` or a list, is rejected before the handler runs (400 or 422), so a handler never receives `nil` or an array where it expected one value. Optional fields are `Option<T>` (or `#[serde(default)]`), which you handle explicitly.

## Validating formats

`Validator::format(field, value, |c| ...)` checks every character of the whole value against a predicate. There is no regular expression engine in Ocre, so Rails' pitfall of `^` and `$` matching at line breaks (a value with a newline and a script after a valid first line) does not exist: the whole value passes or it does not. For position rules, add a `check` (`value.starts_with("https://")`), and for common formats use `email`, `uuid`, `date`, `datetime`, `decimal` and the other `Validator` methods. If you add the `regex` crate yourself, anchor patterns with `\A` and `\z` (or `^`/`$` without the multi-line flag), which match only the start and end of the text.

## Expiry and remember me

`session.expire_in(seconds)?` stores an expiry time inside the encrypted cookie; after it the session starts empty (a copied or replayed cookie stops working too). `session.remember_for(seconds)?` does the same and makes the cookie persistent (`Max-Age`), for "Remember me". `session.expires_at()?` reads it; `session.clear()?` removes it. Keep balances, nonces and other state that must not be replayed in D1, not in the cookie. Server-side revocation of individual sessions is generated by `ocre g auth --db-sessions`.

## Rotating SECRET_KEY_BASE without signing everyone out

List old values (newest first, comma-separated) in the `SECRET_KEY_BASE_PREVIOUS` secret: cookies encrypted with them are still read and re-encrypted with the current key, and JWTs signed with them still verify. Worker secrets cannot be read back, so keep the value you replace. In `.prod.vars` (git-ignored):

```text
SECRET_KEY_BASE_PREVIOUS=<the current value>
SECRET_KEY_BASE=<a new value from `ocre secret`>
```

```sh
ocre secrets push SECRET_KEY_BASE_PREVIOUS SECRET_KEY_BASE --file .prod.vars
```

Remove `SECRET_KEY_BASE_PREVIOUS` after the longest session lifetime. `ocre secrets list` shows which secrets are set locally and in production; `ocre secrets push NAME... --file .prod.vars` uploads values.

## Content-Security-Policy and Permissions-Policy

Generated apps add both in `src/lib.rs` (`content_security_policy()` and `permissions_policy()`): scripts only from the app and `unpkg.com` (htmx), no inline scripts or `on*=` handlers, `object-src 'none'`, `frame-ancestors 'self'`; camera, microphone, geolocation, payment and USB off. Policies are `ocre::security::ContentSecurityPolicy` and `PermissionsPolicy` values added with `.layer(...)`; a handler's own header, or a nested router's layer, overrides them for a route. `.report_only()` sends `Content-Security-Policy-Report-Only`; `.report_uri("/csp-reports")` / `.report_to("csp")` collect violations. For an inline script add `NONCE` to `script_src`, take `nonce: ocre::security::CspNonce` in the handler and write `<script nonce="{{ nonce }}">` (a new random nonce per request).

## Rate limiting

`ocre::security::rate_limit(&ctx, "BINDING", &key).await?` counts one request for `key` against a [Workers Rate Limiting binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) and returns `Error::TooManyRequests` (429, "Too many requests. Try again later.") when the key is over the binding's limit. Rails' `rate_limit to:, within:, by:` maps onto the binding's `limit`, its `period` (10 or 60 seconds) and the key you build. `ocre g auth` already limits its login, sign-up, emailed-link, token and deletion routes with the `AUTH_RATE_LIMITER` binding (see [Authentication](authentication.md#rate-limiting)).

For your own route, add a binding with its own name and `namespace` to `worker.env` of `cloudflare.config.ts`:

```ts
// `namespace`: any integer, unique in your Cloudflare account
FEEDBACK_RATE_LIMITER: bindings.rateLimit({ namespace: "4242", simple: { limit: 3, period: 60 } }),
```

and call it before the expensive part of the handler:

```rust,check
// src/feedback.rs
use axum::{
    Form, Router,
    extract::State,
    http::HeaderMap,
    response::Redirect,
    routing::post,
};
use ocre::{Ctx, Error, Result, Session, security::rate_limit};
use serde::Deserialize;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/feedback", post(create))
}

#[derive(Deserialize)]
struct FeedbackForm {
    message: String,
}

async fn create(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    session: Session,
    Form(form): Form<FeedbackForm>,
) -> Result<Redirect> {
    // One counter per client IP address (Cloudflare's CF-Connecting-IP).
    let ip = ocre::remote_ip(&headers).map_or_else(|| "unknown".to_owned(), |ip| ip.to_string());
    rate_limit(&ctx, "FEEDBACK_RATE_LIMITER", &format!("feedback:{ip}")).await?;
    if form.message.trim().is_empty() {
        return Err(Error::bad_request("write a message"));
    }
    // ... store or email the message ...
    session.flash("notice", "Thanks for your feedback.")?;
    Ok(Redirect::to("/"))
}
```

Past three requests a minute from the same address, `POST /feedback` gets the 429 error page (JSON handlers answer `{"error":{"status":429,"message":"Too many requests. Try again later."}}`). Key by user id (`format!("export:{}", user.id)`) for signed-in actions. The binding is on the free plan, costs no D1, KV or Durable Object operation, and `ocre dev` simulates it. Counters are per Cloudflare location and approximate, so it is a brake, not an exact quota. A missing binding is a 500 whose log names the `bindings.rateLimit(...)` entry to add.

## HTTPS and HSTS

Workers are reached over HTTPS: `*.workers.dev` and custom domains get certificates from Cloudflare. On HTTPS requests Ocre adds `Strict-Transport-Security: max-age=63072000` (two years) and marks the session cookie `Secure`. What Rails' `force_ssl` does beyond that is configured outside the app or by hand:

- **Redirect HTTP to HTTPS**: on a custom domain, turn on Always Use HTTPS in the Cloudflare dashboard (SSL/TLS > Edge Certificates); Cloudflare then redirects before the Worker runs.
- **Other HSTS options** (`includeSubDomains`, `preload`): set the header yourself, in a handler or a layer in `src/lib.rs`; a header set by the app wins over Ocre's.

## Files: downloads and uploads

- **Sending data** (Rails' `send_data`): `ocre::storage::send_data(bytes, "report.csv", "text/csv", Disposition::Download)` sets `Content-Type`, `Content-Length` and a `Content-Disposition` with a cleaned file name. Types a browser could run (HTML, SVG, XML, JavaScript) are sent as `application/octet-stream`, even with `Disposition::Inline`. When cells come from users, prefix values starting with `=`, `+`, `-` or `@` with `'` so spreadsheets do not run them as formulas.
- **Sending stored files** (Rails' `send_file`): files live in R2, not on a disk, so there is no path to traverse. `ocre::storage::serve(&ctx, &attachment, &headers, Disposition::Inline)` streams an attachment by its random key, with the same type rules, `Range` and `If-None-Match`.
- **Upload names**: the uploader's file name loses its directories (`../../x`, `C:\x`) and control characters and is cut to 200 characters; it is only shown in `Content-Disposition`, never used as the storage key (`<prefix>/<22 random characters>`). The R2 bucket is not public and nothing in it is executed; files reach browsers only through your handlers, so check that the user may see the record first. See [File storage](files.md).

## User-supplied CSS and markup

- `ocre::security::sanitize` drops `style` attributes and `<style>` elements, so user HTML cannot restyle the page (CSS injection, as in the MySpace worm) or load images through `url(...)`. Keep `style` out of the lists you pass to `sanitize_with`.
- Never put user text inside a `<style>` element or a `style="..."` attribute; offer choices from a fixed list instead (a theme name that selects one of your CSS classes).
- Markup languages (Markdown, Textile): convert with a library of your choice, then pass the resulting HTML through `sanitize` before inserting it with `|safe`, since these converters let raw HTML through.
- The generated Content-Security-Policy is a second layer if some HTML slips through: scripts only from the app and `unpkg.com`, no `javascript:` URLs or `on*=` handlers, and fonts only from the app, so injected CSS cannot run code or load remote fonts (it still allows inline styles, which the generated layout uses).

## Shell commands

A Worker cannot start processes: there is no `system`, `exec` or shell, so command-line injection has no target. Code that must call another program does it over HTTP (`worker::Fetch`) with arguments encoded as JSON or form data, never pasted into a command string on the other side.

## Dependency and code scanning

Rails runs Brakeman and bundler-audit by default. For an Ocre app, run in the app directory, for example in CI next to `cargo test`:

```sh
cargo install cargo-audit --locked
cargo audit                              # dependencies with known vulnerabilities (RustSec advisory database)
cargo clippy --target wasm32-unknown-unknown -- -D warnings   # Rust lints on the Worker code
```

`cargo deny check advisories` (from `cargo-deny`) does the same check as `cargo audit` and can also enforce licenses and banned crates. Enable Dependabot or Renovate on the repository to get pull requests for updated crates. Ocre does not set these up in new apps.

## More helpers

| Need | Use |
|---|---|
| Only answer on your own host names (Rails' `config.hosts`) | `ALLOWED_HOSTS: bindings.text("example.com, .example.com")` in `cloudflare.config.ts`; other hosts get 403 (localhost always passes) |
| Redirect to a URL taken from the request | `ocre::security::url_from(&uri, &target)` returns it only for paths or URLs of this app (no open redirect, no CR/LF) |
| Show user HTML | `ocre::security::sanitize(&html)` (Rails' safe list) or `sanitize_with`, `strip_tags`; insert with `\|safe` |
| Data in a `<script>` | `ocre::security::json_escape`, `escape_javascript` |
| Log parameters | `ocre::security::filter_parameters(query)` / `filter_json(&value)` hide passwords, tokens, keys, emails |
| Password-protect a staging page | `ocre::security::BasicAuth` extractor, `auth.matches(user, password)`, `BasicAuth::challenge()` |

## What is not included

- **A `force_ssl` switch**: see [HTTPS and HSTS](#https-and-hsts).
- **Roles, two-factor authentication and account lockout**: see [Authentication](authentication.md#what-is-not-included).
- **Dependency scanning in new apps**: see [Dependency and code scanning](#dependency-and-code-scanning).

## See also

- [Authentication](authentication.md): users, login, JWTs, API keys, ownership checks.
- [Security model](../explanations/security-model.md): why Ocre chose these defaults.
- [Configuration](../reference/configuration.md): `SECRET_KEY_BASE`, `ALLOWED_ORIGINS` and other variables.
- [Controllers and routing](controllers.md) and [Views, helpers and forms](views.md): handlers, templates and forms.
- [`ocre secret`](../reference/cli.md#ocre-secret) and [`ocre deploy`](../reference/cli.md#ocre-deploy).
- Rust API: [`Session`](/api/ocre/struct.Session.html), [`Flash`](/api/ocre/struct.Flash.html), [`ALLOWED_ORIGINS`](/api/ocre/constant.ALLOWED_ORIGINS.html), or the [API index](../api-index.md).
