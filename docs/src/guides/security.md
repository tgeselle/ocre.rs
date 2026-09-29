# Sessions, flash and security

This guide shows how an Ocre app keeps per-visitor state in an encrypted session cookie, shows one-time flash messages, and what `ocre::serve` does for every request to protect it: cross-site request (CSRF) checks, CORS, security headers and cookie flags. It ends with what is not included, so you know what to add yourself.

## Before you start

- An app created with [`ocre new`](../reference/cli.md#ocre-new). Everything on this page is built into the `ocre` crate and applied by `ocre::serve` in `src/lib.rs`; no generator is needed.
- The `SECRET_KEY_BASE` secret (see [Configuration](../reference/configuration.md#secret_key_base)). `ocre new` writes a random one to `.dev.vars` for `ocre dev`; `ocre deploy` uploads a new one to the Worker the first time (see [Keys and rotation](#keys-and-rotation)).
- The outputs below come from an app created with `ocre new shop --starter blog`, served by `ocre dev` on the default port 8787.

## What every request goes through

`ocre::serve(routes(), req, env)` wraps the app's router with four layers, outermost first:

| Order | Layer | Effect |
|---|---|---|
| 1 | Security headers | Adds `nosniff`, `SAMEORIGIN` framing, a referrer policy and more to every response; HSTS on HTTPS |
| 2 | CORS | Only when `ALLOWED_ORIGINS` is set: answers preflights and adds `Access-Control-*` headers for those origins |
| 3 | Cross-origin protection (CSRF) | Refuses unsafe requests a browser sends from another site, with 403 |
| 4 | Session | Decrypts the session cookie on first use and sends `Set-Cookie` when the handler changed it |

None of these layers makes a D1, KV or network call: they cost a little CPU and no free-plan quota.

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

- **Encrypted and authenticated.** The cookie value is JSON encrypted with AES-256-GCM, with a key derived from `SECRET_KEY_BASE`. Clients can neither read nor change it. A cookie that does not decrypt (tampered, truncated, or encrypted with an older key) is ignored: the request starts with an empty session, no error.
- **4 KB at most.** Browsers drop cookies over 4096 bytes (name, value and attributes). When a change would make the cookie larger, the response is a 500 and the log says `the session cookie would be <n> bytes; browsers drop cookies over 4096. Fix: store ids in the session, not records`. Store ids and short strings, never records, and never secrets such as tokens or passwords.
- **Cookie flags.** The cookie is `_ocre_session`, `Path=/`, `HttpOnly` (JavaScript cannot read it), `SameSite=Lax`, and `Secure` when the request came over HTTPS (always on `*.workers.dev`; not on `http://localhost`). It has no `Max-Age`: it lasts until the browser session ends or the app clears it.
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

## Keys and rotation

`SECRET_KEY_BASE` must be set and at least 64 characters long. `ocre secret` prints a new random value (128 hex characters):

```sh
ocre secret
```

Where it comes from:

| Environment | Source |
|---|---|
| `ocre dev` | `.dev.vars` (git-ignored), written by `ocre new` |
| Production | A Worker secret. `ocre deploy` checks `wrangler secret list` and, when the Worker has no `SECRET_KEY_BASE`, uploads a new random one with the deploy; it never replaces an existing one |

Two keys are derived from it: the session cookie key, and (with a fixed label) the HS256 key of `ocre::jwt` (see [Authentication](authentication.md#json-clients-jwts-and-api-keys)). When the secret is missing or shorter than 64 characters, requests that write the session, or read a session cookie they received, answer 500, and the log names the fix: ``the SECRET_KEY_BASE secret is not set. Fix: run `ocre secret`, put the value in .dev.vars as SECRET_KEY_BASE=... for `ocre dev` (`ocre new` does this), and deploy with `ocre deploy`, which uploads it``.

**Rotating the secret signs everyone out.** Existing cookies no longer decrypt, so every visitor starts with an empty session, and every JWT fails to verify. This is the way to invalidate all sessions at once (there is no server-side session list). To rotate in production:

```sh
ocre secret | npx wrangler secret put SECRET_KEY_BASE
```

Changing the value in `.dev.vars` and restarting `ocre dev` shows the effect: a browser that was signed in is redirected to `/login` by the next `CurrentUser` page, and an old JWT gets `401 {"error":{"message":"Unauthorized","status":401}}`.

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
- htmx requests (`hx-post`, `hx-delete`...) are same-origin fetches and pass.
- The session cookie is also `SameSite=Lax`: browsers do not send it with cross-site `POST`s at all, a second layer for the same attack.

## ALLOWED_ORIGINS: another site calling the app

When a separate frontend (another domain, a local dev server on another port) must call the app from the browser, list its origins in the `ALLOWED_ORIGINS` Worker variable, comma-separated, under `[vars]` in `wrangler.toml` (or in `.dev.vars` for `ocre dev`):

```toml
# wrangler.toml
[vars]
ALLOWED_ORIGINS = "https://app.example.com, https://admin.example.com"
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

## SQL parameters

D1 queries take `?1, ?2` placeholders and a `params![...]` list; the values never become SQL text, so they cannot inject SQL:

```rust
ctx.db()?.first("SELECT * FROM users WHERE email = ?1", params![email]).await
```

Never build SQL with `format!` from user input. When a column or sort order comes from the request, match it against a fixed list of allowed names first.

## What is not included

Checked against the `ocre` crate: these protections are not built in.

| Missing | What to do |
|---|---|
| Rate limiting | Login, sign-up, password-reset and token routes accept unlimited attempts. Add [Cloudflare rate limiting rules](https://developers.cloudflare.com/waf/rate-limiting-rules/) for them before going public (see [Authentication](authentication.md#before-going-public)) |
| Content-Security-Policy | No CSP header is sent. Set one per response in a handler, or with an axum middleware in `routes()`. Generated layouts load htmx from `unpkg.com`, so a CSP must allow that origin (or serve htmx from `public/`) |
| Server-side session revocation | Sessions last until logout, the browser session ends, or `SECRET_KEY_BASE` changes (which signs everyone out). There is no "sign out everywhere" for one user |
| Session expiry | The cookie has no expiry time inside it; store a timestamp in the session and check it if you need one |
| `Permissions-Policy`, `Cross-Origin-*-Policy` | Not sent; add them like any header |
| Authentication and authorization | Generated by [`ocre g auth`](authentication.md); records are protected only by the checks your handlers make |

## See also

- [Authentication](authentication.md): users, login, JWTs, API keys, ownership checks.
- [Security model](../explanations/security-model.md): why Ocre chose these defaults.
- [Configuration](../reference/configuration.md): `SECRET_KEY_BASE`, `ALLOWED_ORIGINS` and other variables.
- [Controllers, routing, views and htmx](controllers.md): handlers, templates and forms.
- [`ocre secret`](../reference/cli.md#ocre-secret) and [`ocre deploy`](../reference/cli.md#ocre-deploy).
- Rust API: [`Session`](/api/ocre/struct.Session.html), [`Flash`](/api/ocre/struct.Flash.html), [`ALLOWED_ORIGINS`](/api/ocre/constant.ALLOWED_ORIGINS.html), or the [API index](../api-index.md).
