# Security model

Ocre protects every app by default against session tampering, cross-site request forgery, clickjacking, MIME sniffing and leaked error details, and the code `ocre g auth` generates adds password, token, JWT and API-key handling built on small framework primitives. This page describes each protection as the code implements it, the reasons behind the choices, and what is left to you.

## Before you start

Nothing to install to read this page. The framework parts are in the `ocre` crate (`session.rs`, `protect.rs`, `security.rs` and `security/`, `password.rs`, `token.rs`, `jwt.rs`, `oauth.rs`, `storage.rs`, `realtime.rs`, `error.rs`); the authentication parts exist in an app after `ocre g auth` (`src/auth.rs`, `src/auth_api.rs`, `src/models/user.rs`, `src/models/auth_token.rs`, `src/models/api_key.rs`, and with its options `src/models/user_session.rs` and `src/oauth.rs`). Everything depends on one secret, `SECRET_KEY_BASE`, which `ocre new` writes to `.dev.vars` and `ocre deploy` uploads once (see [Configuration](../reference/configuration.md#secret_key_base)).

## At a glance

| Threat | What Ocre does | Where | Your part |
|---|---|---|---|
| Reading or forging the session | Cookie encrypted and authenticated with AES-256-GCM | `ocre::serve` | Keep `SECRET_KEY_BASE` secret |
| Cross-site request forgery | Browsers' cross-site unsafe requests get 403, without tokens | `ocre::serve` | Never change data in a `GET` handler |
| Cross-origin reads by other sites | No CORS headers unless `ALLOWED_ORIGINS` lists the origin | `ocre::serve` | List only origins you control |
| Clickjacking, MIME sniffing | `X-Frame-Options: SAMEORIGIN`, `X-Content-Type-Options: nosniff` and more | `ocre::serve` | none |
| Requests for other host names | 403 for hosts not in `ALLOWED_HOSTS`, when set | `ocre::serve` | List your domains |
| XSS in pages | askama escapes `{{ value }}`; generated full-stack apps send a Content-Security-Policy without inline scripts | templates, `src/lib.rs` | Never mark user input `\|safe` unless it went through `ocre::security::sanitize` |
| SQL injection | Values bind to `?1, ?2` placeholders through `params![...]`; `Query` takes column names as `&'static str` | `Db`, `Query` | Never build SQL with `format!` from input |
| Stolen database: passwords | PBKDF2-HMAC-SHA256 digests, 100,000 iterations | `ocre::password` | none |
| Stolen database: links and keys | Only SHA-256 digests of tokens are stored | `ocre::token`, auth models | Keep it that way for your own tokens |
| JWT algorithm confusion | Only `HS256` is accepted, `none` included in the refusal | `ocre::jwt` | Keep TTLs short |
| Uploaded HTML or SVG running as the app | Served as `application/octet-stream` downloads | `ocre::storage::serve` | Check who may download |
| Cross-site WebSocket hijacking | Handshakes checked like forms | `ocre::serve`, `src/realtime.rs` | Authorize channels in `connect` |
| Leaking internals in errors | Internal messages are logged, clients see `Internal server error` | `ocre::Error` | Use `Error::internal` for unexpected failures |
| Open redirect after login | Only local paths are remembered and followed (`ocre::security::url_from`) | `src/auth.rs` | Use `url_from` for your own redirects |
| Account enumeration | Same answers whether an email has an account; a password hash runs either way | auth controllers, `user.rs` | none |
| Brute force, credential stuffing | 10 attempts a minute per IP address and action on every route that checks a password or sends an email | `src/auth_api.rs` (`throttle`), `ocre::security::rate_limit` | Keep the `AUTH_RATE_LIMITER` binding; limit your own costly routes |

## SECRET_KEY_BASE: one secret, two keys

Both the session key and the JWT key are derived from the `SECRET_KEY_BASE` Worker secret, so an app has a single secret to create, upload and rotate:

- **Session key:** the cookie crate's `Key::derive_from` (HKDF-SHA256) turns the secret into the key of its private (AES-256-GCM) cookie jar.
- **JWT key:** HMAC-SHA256 of the fixed label `ocre/jwt/hs256` keyed with the secret, so it differs from the session key.

`ocre secret` generates 128 hexadecimal characters (512 bits). A value shorter than 64 characters is refused: handlers that use the session or a JWT answer 500, and the log names the fix; other routes keep working.

`ocre deploy` uploads a new secret only when the Worker has none, and never replaces an existing one. Rotating it yourself (a new `ocre secret` value uploaded with `ocre secrets push SECRET_KEY_BASE --file .prod.vars`) signs every user out and invalidates every JWT at once, which is what you want after a leak; API keys and emailed links, stored as digests in D1, keep working. For a planned rotation, put the old value in the `SECRET_KEY_BASE_PREVIOUS` secret first: keys derived from it still decrypt cookies (which are re-encrypted with the new key on the same response) and verify JWTs, but never encrypt or sign anything new. Anyone who learns the secret can forge a session for any user id and mint JWTs: rotate it, without `SECRET_KEY_BASE_PREVIOUS`, if it leaks.

## Sessions

The session is a JSON object stored in the `_ocre_session` cookie, encrypted and authenticated with AES-256-GCM, like Rails' default cookie store. The browser can neither read nor change it; a cookie that does not decrypt (tampered, or made with a key that is neither `SECRET_KEY_BASE` nor listed in `SECRET_KEY_BASE_PREVIOUS`) starts an empty session. Nothing is stored on the server: sessions cost no D1 row and no KV operation.

The cookie is sent with `HttpOnly`, `SameSite=Lax`, `Path=/`, and `Secure` on HTTPS requests. By default it has no `Expires` or `Max-Age`, so browsers treat it as a session cookie; `session.remember_for(seconds)` adds `Max-Age` ("remember me"). A session that would exceed 4,096 bytes fails the request with a 500 whose log says to store ids rather than records.

What a cookie store implies:

- **Revocation needs an expiry or D1.** Logging out clears the browser's cookie, but a copy taken earlier still decrypts. `session.expire_in(seconds)` and `remember_for` store an expiry inside the encrypted cookie, checked on every request, so a copy stops working after it; `ocre g auth` signs users in for two weeks. To end one session, or all of a user's sessions, before that, `ocre g auth --db-sessions` keeps a row per session in D1 and the cookie holds only a random token whose digest finds it: deleting the row signs that device out on its next request.
- **No fixation.** `auth::sign_in` empties the session before storing `user_id` (pending flash messages stay), so nothing from before the login carries over, and there is no server-side session id to fix in advance.
- **Ids only.** The generated auth code stores `user_id` (or the session token) and, while redirecting to the login page, the page to return to. Put ids in the session, never records, balances, nonces or secrets: an old copy of the cookie could be replayed until it expires.

## Cross-site request forgery without tokens

Forms carry no CSRF token. Instead, Ocre refuses unsafe requests that a browser sends on behalf of another site, using the headers browsers add to every request. This is the check Go 1.25 ships as `http.CrossOriginProtection`. For each request, in order:

1. `GET`, `HEAD` and `OPTIONS` pass, except WebSocket handshakes (`Upgrade: websocket`), which are checked like forms.
2. An `Origin` listed in `ALLOWED_ORIGINS` passes.
3. `Sec-Fetch-Site: same-origin` or `none` (typed in the address bar, a bookmark) passes; any other value, `same-site` included, gets 403 `Forbidden: cross-site request. Add the origin to ALLOWED_ORIGINS to allow it.`
4. Without `Sec-Fetch-Site` (older browsers): when both `Origin` and `Host` are present, the host in `Origin` must equal `Host`, else 403 `Forbidden: cross-origin request. Add the origin to ALLOWED_ORIGINS to allow it.`
5. A request with neither header does not come from a browser page (curl, a mobile app, a server) and passes: it cannot carry a victim's cookies by accident.

`SameSite=Lax` on the session cookie is a second layer: browsers do not send it with cross-site `POST`s at all.

Consequences:

- A `GET` handler must never change data: it is not checked.
- Subdomains are other sites here: a form on `admin.example.com` posting to `example.com` is `same-site`, not `same-origin`, and gets 403 unless listed in `ALLOWED_ORIGINS`.
- JSON clients authenticated with a Bearer token are not affected (they are not browsers, or send no cookie).

## CORS

Without `ALLOWED_ORIGINS`, `ocre::serve` adds no CORS layer: browsers let other sites send requests but not read the answers, and the CSRF check refuses their unsafe ones. With it (a comma-separated `bindings.text` variable such as `"https://app.example.com, https://admin.example.com"`), the listed origins get CORS headers for `GET`, `POST`, `PUT`, `PATCH` and `DELETE`, with the `Content-Type`, `Authorization` and `Accept` request headers, and credentials allowed. Credentials mean cookies: a listed origin can act as the signed-in user, and it also passes the CSRF check. List only frontends you control.

## Security headers

Every response gets these headers unless the handler set its own value:

| Header | Value |
|---|---|
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `SAMEORIGIN` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |
| `X-XSS-Protection` | `0` (turns off the obsolete browser filter) |
| `X-Permitted-Cross-Domain-Policies` | `none` |
| `Strict-Transport-Security` | `max-age=63072000` (two years), on HTTPS requests only |

Full-stack apps made by `ocre new` also add a `Content-Security-Policy` (scripts only from the app and `unpkg.com`, no inline scripts or `on*=` handlers, `object-src 'none'`, `frame-ancestors 'self'`) and a `Permissions-Policy` (camera, microphone, geolocation, payment and USB off) as layers in `src/lib.rs`, built with `ocre::security::ContentSecurityPolicy` and `PermissionsPolicy`; a handler's own header or a nested router's layer overrides them. API-only apps get neither, since they serve no HTML.

## Host authorization

When the `ALLOWED_HOSTS` variable is set (`"example.com, .example.com"`), a request whose `Host` is not listed gets `403 Forbidden: blocked host` before the session, the CSRF check or any handler runs; a leading `.` allows subdomains, and `localhost`, `127.0.0.1` and `[::1]` always pass for `ocre dev`. DNS rebinding cannot reach a Worker (Cloudflare routes only your own host names to it), so the point on Workers is to answer only on your domain and not on `workers.dev` or preview URLs.

## Passwords

`ocre::password::hash` produces `pbkdf2_sha256$100000$<salt>$<hash>`: PBKDF2-HMAC-SHA256 with 100,000 iterations, a 16-byte random salt and a 32-byte hash (base64). `verify` recomputes it with the digest's own salt and count and compares in constant time.

Why PBKDF2: on the free plan a request has 10 ms of CPU. bcrypt or argon2 compiled to WebAssembly would not fit; WebCrypto's `crypto.subtle.deriveBits` runs PBKDF2 natively in workerd, outside the WebAssembly module. One hash was measured at 5.5 ms of CPU. Workers cap PBKDF2 at 100,000 iterations, below OWASP's 600,000 recommendation for this algorithm; the count is stored in each digest, so it can grow later (`ocre::password::iterations(digest)` reads it back).

In the generated auth code, passwords must be 8 to 128 characters, emails are trimmed and lowercased, `User` never serializes `password_digest`, and a login with an unknown email runs a hash anyway so response times do not reveal which emails have accounts. Errors name the failed step, never the password.

## Emailed tokens: password reset and magic links

- `ocre::token::generate()` returns 32 random bytes (256 bits) as 43 URL-safe characters.
- The `auth_tokens` table keeps only `ocre::token::digest(token)`, its SHA-256 in hex: a leaked database holds no working link. A fast hash is enough for long random values.
- A token is valid 15 minutes and works once: it is consumed by a single `DELETE ... RETURNING`, so two concurrent uses cannot both succeed. Issuing a new link cancels the user's previous one of the same purpose.
- The emailed link opens a page with a button that POSTs, so mail scanners that follow links do not use it up.
- The request forms answer the same ("If an account exists for that email, ...") whether or not the email has an account.
- Links use the request's origin; Cloudflare routes requests by host name, so it is one of the app's own hosts.

## JWT

`ocre::jwt` issues and checks HS256 tokens with `sub` (the user id), `iat` and `exp`; `ocre g auth` issues them from `POST /api/auth/token` with a one-hour lifetime. `decode` checks the shape, then that the header's `alg` is exactly `HS256` (a token naming `none` or any other algorithm is refused before its signature is looked at), then the signature in constant time, then `exp` against the current time. Every failure is the same 401, without saying which check failed.

JWTs cannot be revoked before they expire; `BearerUser` looks the user up on every request, so a deleted user's tokens stop working. For long-lived access, use API keys.

## API keys

`POST /api/auth/keys` creates a key with `ocre::token::generate()` and returns it once; the `api_keys` table stores its SHA-256 digest (unique index), with `user_id`, `name` and `last_used_at`. `BearerUser` treats a Bearer value containing a dot as a JWT and anything else as an API key, looked up by digest. Keys are revoked with `DELETE /api/auth/keys/{id}`, which only deletes the caller's own keys. `last_used_at` is written at most once an hour, to save D1 writes.

## Authorization is your code

Ocre authenticates; deciding what a user may see is application code. `CurrentUser` (HTML) redirects visitors to `/login` and brings them back afterwards (GET requests only, local paths only: `//evil.example` and `https://...` are ignored). `BearerUser` (JSON) answers 401. Filter every query by the owner:

```rust,check
// src/notes.rs: a page only its owner may see. After `ocre g auth`;
// assumes a `notes` table with `id`, `user_id` and `body` columns.
use axum::{
    Router,
    extract::{Path, State},
    routing::get,
};
use ocre::{Ctx, OptionExt, Result, params};
use serde::Deserialize;

use crate::auth::CurrentUser;

#[derive(Deserialize)]
struct Note {
    id: i64,
    body: String,
}

pub fn routes() -> Router<Ctx> {
    Router::new().route("/notes/{id}", get(show))
}

/// Another user's note is a 404, like a missing one: its existence is not revealed.
async fn show(State(ctx): State<Ctx>, CurrentUser(user): CurrentUser, Path(id): Path<i64>) -> Result<String> {
    let note: Note = ctx
        .db()?
        .first("SELECT id, body FROM notes WHERE id = ?1 AND user_id = ?2", params![id, user.id])
        .await?
        .or_404()?;
    Ok(format!("Note {}: {}", note.id, note.body))
}
```

`Error::Forbidden` (403) is there for checks that should say "not allowed" rather than "not found". There are no roles or permissions.

## Files

- **Keys.** Objects are stored under `<prefix>/<22 random characters>` (128 bits), never derived from the file name, and never reused.
- **Names.** The uploader's file name loses its directories and control characters and is cut to 200 characters; it is only used in `Content-Disposition` (ASCII `filename=` plus UTF-8 `filename*=`, RFC 6266).
- **Types.** Uploads are checked against the model's exact allowlist (`Rules::content_types`, no wildcards: `image/*` would admit SVG). The type comes from the browser; nothing sniffs file contents.
- **Serving.** `storage::serve` shows inline only types that cannot run scripts (raster images, PDF, plain text, audio, video). HTML, SVG, XML and JavaScript are sent as `application/octet-stream` attachments, and every other type as an attachment, so an uploaded file never runs in the app's origin. Responses carry `Cache-Control: private, no-cache` and `nosniff`.
- **Access.** A file route is as protected as its handler: check that the user may see the record before calling `serve`.

## Realtime

A WebSocket handshake is a `GET`, but browsers send cookies with it and let any site open one, so Ocre checks it like a form: a handshake from another site gets 403. In the app, `src/realtime.rs` routes `GET /realtime/{channel}` to `connect`, which lists the channels that may be opened (others are 404) and can require a user (`CurrentUser` works, since the session cookie comes with the handshake). Channel names must be 1 to 128 ASCII letters, digits, `_`, `-`, `.` or `:` (400 otherwise). Everyone connected to a channel receives every broadcast, so private data needs one channel per user or record. What clients send is ignored, and broadcasts are HTML: render them with askama so they are escaped.

## Errors

`Error::Internal` (and any `worker::Error` converted with `?`) is logged as `[ocre] <message>` and answered as `Internal server error`, in HTML pages, JSON (`{"error": {"status": 500, "message": "Internal server error"}}`) and GraphQL alike. A failed D1 query logs D1's error and the SQL with its `?N` placeholders; Ocre does not add the bound values. Client errors (400, 404, 413, 422, 429) show their message, which you choose. To log request parameters yourself, pass them through `ocre::security::filter_parameters` or `filter_json`, which replace passwords, tokens, keys and similar values with `[FILTERED]`.

## OAuth sign-in

`ocre g auth --oauth github,google` uses the authorization code flow with PKCE (`ocre::oauth`). The random `state` and the PKCE verifier are kept in the encrypted session between the redirect and the callback; the callback compares the state in constant time, so another site cannot sign a visitor in to the attacker's account (login CSRF). The client secret stays in a Worker secret and is only sent from the Worker to the provider's token endpoint. An existing account is linked only through an email address the provider reports as verified, and a new account created that way has no password.

## What Ocre does not do yet

- **Sign out everywhere** needs `ocre g auth --db-sessions`; cookie sessions end at logout, at their expiry (`expire_in`) or when `SECRET_KEY_BASE` changes.
- **Roles and permissions.** Authorization is the ownership checks you write.
- **Two-factor authentication and account lockout.** Rate limiting slows guessing; it does not lock an account.
- **Encrypted or signed cookies other than the session.** Put such values in the session.

## See also

- [Sessions, flash and security](../guides/security.md): using sessions, flash, `ALLOWED_ORIGINS` in an app
- [Authentication](../guides/authentication.md): what `ocre g auth` generates and how to use it
- [File storage](../guides/files.md) and [Realtime](../guides/realtime.md)
- [Deployment](../guides/deployment.md): secrets in production and rate limiting rules
- [Configuration](../reference/configuration.md): [`SECRET_KEY_BASE`](../reference/configuration.md#secret_key_base), [`ALLOWED_ORIGINS`](../reference/configuration.md#allowed_origins)
- Rustdoc: [`ocre::password`](/api/ocre/password/index.html), [`ocre::token`](/api/ocre/token/index.html), [`ocre::jwt`](/api/ocre/jwt/index.html)
