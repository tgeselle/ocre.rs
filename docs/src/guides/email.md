# Email

This guide sends email from an Ocre app with `ocre::mail` (built directly, or by a mailer generated with `ocre g mailer`), picks a delivery adapter for development and production, sends from the background with `deliver_later`, and receives email through Cloudflare Email Routing with `ocre g mailbox`.

## Before you start

- An app created with [`ocre new`](../reference/cli.md#ocre-new). It already has what sending needs: `MAIL_FROM` under `[vars]` in `wrangler.toml` and `MAIL_ADAPTER=log` in `.dev.vars`, so `ocre dev` prints every email instead of sending it.
- Mailers: [`ocre g mailer`](../reference/generators.md#ocre-g-mailer). Receiving: [`ocre g mailbox`](../reference/generators.md#ocre-g-mailbox). Sending later: one [`ocre g job`](../reference/generators.md#ocre-g-job) (it wires the queue `deliver_later` uses).
- For production delivery: a domain you control, verified with Resend or onboarded to Cloudflare Email Service (see [Choose an adapter](#choose-an-adapter)).
- The outputs below come from an app created with `ocre new shop --starter blog`, served by `ocre dev` on the default port 8787.

## Send an email

Build an `ocre::mail::Email` and hand it to `ocre::mail::send`:

```rust
use ocre::mail::Email;

let email = Email::new("ada@example.com", "Welcome", "Hello Ada,\n\nhttps://example.com/start\n")
    .html("<p>Hello Ada,</p><p><a href=\"https://example.com/start\">Start</a></p>")
    .reply_to("support@example.com");
ocre::mail::send(&ctx, email).await?;
```

| `Email` | Meaning |
|---|---|
| `Email::new(to, subject, text)` | One recipient, a one-line subject, a plain-text body (always sent) |
| `.html(html)` | Adds an HTML body, shown by clients that render HTML. Sent as given: escape user input (askama templates do) |
| `.reply_to(address)` | Replies go there instead of to `MAIL_FROM` |
| Fields `to`, `subject`, `text`, `html`, `reply_to` | Public, for reading or changing an email a mailer built |

Nothing is checked while building. `send` checks, then delivers:

| Problem | Result |
|---|---|
| `to` or `reply_to` is not an email address | `Error::BadRequest`, 400 `invalid email address: <address>` |
| `MAIL_ADAPTER` unset or unknown, `MAIL_FROM` unset or not an address, subject empty or on several lines, `RESEND_API_KEY` or the `EMAIL` binding missing, the provider refused or could not be reached | `Error::Internal`, 500; the log line says what to fix |

The sender is the `MAIL_FROM` variable, `noreply@yourdomain.com` or `Name <noreply@yourdomain.com>`. `ocre new` puts a placeholder in `wrangler.toml`:

```toml
# wrangler.toml
[vars]
# Sender for `ocre::mail::send`: "noreply@yourdomain.com" or "Name <noreply@yourdomain.com>".
MAIL_FROM = "shop <noreply@example.com>"
```

`send` waits for the provider (one HTTP subrequest with Resend). The request answers after delivery succeeded; use [`deliver_later`](#send-from-the-background-deliver_later) to answer first and retry failures.

## Choose an adapter

The `MAIL_ADAPTER` variable names how mail leaves the Worker. Nothing is guessed from which keys happen to be set, so a development machine holding a real API key never sends by accident.

| `MAIL_ADAPTER` | Delivery | Configuration | Free-plan limits (September 2026) |
|---|---|---|---|
| `log` | Prints the whole email (headers, text, HTML) to the Worker console between `[ocre mail]` lines; sends nothing | none; `ocre new` writes `MAIL_ADAPTER=log` to `.dev.vars`, which overrides `[vars]` in `ocre dev` | none |
| `resend` | `POST https://api.resend.com/emails` | `RESEND_API_KEY` secret; `MAIL_FROM` on a domain verified in Resend | [Resend free plan](https://resend.com/docs/knowledge-base/account-quotas-and-limits): 100 emails a day, 3,000 a month, one domain; any recipient |
| `cloudflare` | Cloudflare Email Service through the `EMAIL` [send_email binding](https://developers.cloudflare.com/email-service/api/send-emails/workers-api/) | `[[send_email]] name = "EMAIL"` in `wrangler.toml`; `MAIL_FROM` on a domain onboarded to Email Service | [Workers Free](https://developers.cloudflare.com/email-service/platform/pricing/): only verified destination addresses of the account; any recipient needs Workers Paid (3,000 a month included, then $0.35 per 1,000) |

For sign-up, magic-link and password-reset mail to any address on the free plan, use Resend. The `cloudflare` adapter on the free plan suits mail to yourself (alerts, reports).

With `MAIL_ADAPTER` unset, `send` fails rather than dropping mail silently. With `MAIL_ADAPTER` removed from `.dev.vars`, a handler that sends answers 500 and `ocre dev` logs:

```text
✘ [ERROR] [ocre] cannot send email: MAIL_ADAPTER is not set. Fix: set MAIL_ADAPTER to "resend" (with the RESEND_API_KEY secret) or "cloudflare" (with a [[send_email]] binding named EMAIL) under [vars] in wrangler.toml; `ocre new` puts MAIL_ADAPTER=log in .dev.vars so `ocre dev` only logs mail
```

### log (development)

This is what `ocre dev` prints for an email with a text and an HTML body:

```text
[ocre mail] not sent (MAIL_ADAPTER = "log")
From: shop <noreply@example.com>
To: bob@example.com
Subject: Welcome

Hello bob@example.com,

This is the welcome email. Edit templates/mailers/user/welcome.txt.
[ocre mail] HTML version:
<!DOCTYPE html>
<html>
<body>
<p>Hello bob@example.com,</p>
<p>This is the welcome email. Edit templates/mailers/user/welcome.html.</p>
</body>
</html>
[ocre mail] end
```

Links in emails (magic links, password resets) can be opened from there. In production, `log` writes to the Worker logs (`npx wrangler tail`); it never delivers.

### resend (production, any recipient)

1. Create a [Resend](https://resend.com) account, add and verify your domain at <https://resend.com/domains>, and create an API key at <https://resend.com/api-keys>.
2. Store the key as a Worker secret:

   ```sh
   npx wrangler secret put RESEND_API_KEY
   ```

3. In `wrangler.toml`, set the adapter and a sender on the verified domain:

   ```toml
   [vars]
   MAIL_FROM = "Shop <noreply@yourdomain.com>"
   MAIL_ADAPTER = "resend"
   ```

4. `ocre deploy`. `.dev.vars` keeps `MAIL_ADAPTER=log`, so `ocre dev` still only prints. To send for real from `ocre dev`, put `MAIL_ADAPTER=resend` and `RESEND_API_KEY=...` in `.dev.vars`.

Resend errors are 500s whose log names the fix: a 401 or 403 from Resend says to check `RESEND_API_KEY` and the domain of `MAIL_FROM`; a 429 says the rate or daily quota was reached.

### cloudflare (production, verified addresses on the free plan)

1. Onboard the sending domain to [Cloudflare Email Service](https://developers.cloudflare.com/email-service/) in the dashboard.
2. In `wrangler.toml`, uncomment the binding `ocre new` left there, and set the adapter:

   ```toml
   [vars]
   MAIL_FROM = "Shop <noreply@yourdomain.com>"
   MAIL_ADAPTER = "cloudflare"

   [[send_email]]
   name = "EMAIL"
   ```

3. `ocre deploy`.

`ocre dev` simulates the binding: with `MAIL_ADAPTER=cloudflare` in `.dev.vars` and the binding uncommented, sending prints wrangler's summary and writes the bodies to files instead of sending:

```text
[wrangler:info] send_email binding called with MessageBuilder:
From: "shop" <noreply@example.com>
To: bob@example.com
Subject: Welcome

Text: /private/tmp/webguides.5ujL/shop/.wrangler/tmp/email/miniflare-4210ceb107aa3f4fd67fad52fa59ebb0/email-text/MgiGstxS0hievJ5wjXK6kUAtkLUcdzErHDZS@example.com.txt
HTML: /private/tmp/webguides.5ujL/shop/.wrangler/tmp/email/miniflare-4210ceb107aa3f4fd67fad52fa59ebb0/email-html/MgiGstxS0hievJ5wjXK6kUAtkLUcdzErHDZS@example.com.html
```

In production a refused email is a 500 whose log explains that `MAIL_FROM` must use an onboarded domain and that the free plan only sends to verified destination addresses.

## Mailers

A mailer is a module of functions that each build one `Email` from templates, like Rails' Action Mailer. Generate one with the name and its actions:

```sh
ocre g mailer User welcome password_reset
```

```text
  create  src/mailers/user.rs
  create  templates/mailers/user/welcome.txt
  create  templates/mailers/user/welcome.html
  create  templates/mailers/user/password_reset.txt
  create  templates/mailers/user/password_reset.html
  create  src/mailers/mod.rs
  update  src/lib.rs

Next:
  send it from a handler: ocre::mail::send(&ctx, mailers::user::welcome(&address)?).await?
  ocre dev (MAIL_ADAPTER=log in .dev.vars prints each email instead of sending it)
```

`src/mailers/user.rs` has one function per action. Each renders a `.txt` and an `.html` template with askama:

```rust
#[derive(Template)]
#[template(path = "mailers/user/welcome.txt")]
struct WelcomeText<'a> {
    to: &'a str,
}

#[derive(Template)]
#[template(path = "mailers/user/welcome.html")]
struct WelcomeHtml<'a> {
    to: &'a str,
}

/// "Welcome" email to `to`.
pub fn welcome(to: &str) -> Result<Email> {
    let text = WelcomeText { to }.render()?;
    let html = WelcomeHtml { to }.render()?;
    Ok(Email::new(to, "Welcome", text).html(html))
}
```

```text
Hello {{ to }},

This is the welcome email. Edit templates/mailers/user/welcome.txt.
```

The subject is the humanized action name ("Welcome", "Password reset"); change it in the function. `.txt` templates are not HTML-escaped (they are plain text); `.html` templates are. In an API-only app (`ocre new --api`), the mailer has no templates: each function builds the text with `format!` and sends no HTML; add `.html(...)` yourself.

### Add data to an email

Everything an email shows goes through the function's arguments into the template structs. To greet the user by name and link to their account, add the fields to both structs and to the function:

```rust
#[derive(Template)]
#[template(path = "mailers/user/welcome.txt")]
struct WelcomeText<'a> {
    to: &'a str,
    name: &'a str,
    account_url: &'a str,
}

pub fn welcome(to: &str, name: &str, account_url: &str) -> Result<Email> {
    let text = WelcomeText { to, name, account_url }.render()?;
    let html = WelcomeHtml { to, name, account_url }.render()?;
    Ok(Email::new(to, format!("Welcome, {name}"), text).html(html))
}
```

and use them in the templates (`Hello {{ name }},` and `{{ account_url }}`). Pass values (strings, numbers), not a database handle: a mailer only builds the email, the caller loads the data. Links need an absolute URL: in a handler, build it from the request (`crate::auth::origin(&uri)` after `ocre g auth`).

### Send a mailer's email from a handler

Validate user-typed addresses first, so a typo is a form error (422) instead of a 400. This module sends the fixture's `welcome` email as an invitation, now and from the queue; register it with `mod invitations;` under `// ocre:modules` and `.merge(invitations::routes())` under `// ocre:routes` in `src/lib.rs`:

```rust,check
// src/invitations.rs
use axum::{Form, Router, extract::State, response::Redirect, routing::post};
use ocre::{Ctx, Result, Session, Validator};
use serde::Deserialize;

use crate::mailers;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/invitations", post(create)).route("/invitations/later", post(create_later))
}

#[derive(Deserialize)]
struct InvitationForm {
    email: String,
}

/// Sends now: the request waits for the provider (or the console with `MAIL_ADAPTER=log`).
async fn create(State(ctx): State<Ctx>, session: Session, Form(form): Form<InvitationForm>) -> Result<Redirect> {
    let mut v = Validator::new();
    v.email("email", &form.email);
    v.finish()?;
    ocre::mail::send(&ctx, mailers::user::welcome(&form.email)?).await?;
    session.flash("notice", "Invitation sent.")?;
    Ok(Redirect::to("/"))
}

/// Sends from the jobs queue: answers at once, retries provider failures.
async fn create_later(State(ctx): State<Ctx>, session: Session, Form(form): Form<InvitationForm>) -> Result<Redirect> {
    let mut v = Validator::new();
    v.email("email", &form.email);
    v.finish()?;
    ocre::mail::deliver_later(&ctx, mailers::user::welcome(&form.email)?).await?;
    session.flash("notice", "Invitation sent.")?;
    Ok(Redirect::to("/"))
}
```

```sh
curl -s -X POST http://localhost:8787/invitations -d email=bob@example.com -o /dev/null -w '%{http_code}\n'
curl -s -X POST http://localhost:8787/invitations -d email=not-an-email -w '\n%{http_code}\n'
```

```text
303
<h1>422</h1><p>Validation failed</p><ul><li>Email is invalid</li></ul>
422
```

The first request prints the "Welcome" email to bob@example.com shown in [log (development)](#log-development).

## Send from the background: deliver_later

`ocre::mail::deliver_later(&ctx, email).await?` is Rails' `deliver_later`. It checks the email and the configuration right away like `send` (a bad address is still a 400, a missing `MAIL_ADAPTER` or `MAIL_FROM` a 500), puts the email on the jobs queue, and returns without waiting for the provider. The queue consumer sends it with `send` moments later; a failure (Resend down, quota reached) is retried with the jobs backoff (30 s, 1 min, 3 min, 9 min, 27 min), then moved to the dead-letter queue. The Resend key and the `EMAIL` binding are only looked up when the consumer sends.

It needs the `JOBS` queue that the first `ocre g job` adds to `wrangler.toml` (see [Background jobs and schedules](jobs.md)); without it, `deliver_later` is a 500 whose log says to run `ocre g job <Name>` once. In `ocre dev` the queue runs locally, and the email appears within about 5 seconds, followed by the job line:

```sh
curl -s -X POST http://localhost:8787/invitations/later -d email=carol@example.com -o /dev/null -w '%{http_code}\n'
```

```text
303
```

```text
[ocre mail] not sent (MAIL_ADAPTER = "log")
From: shop <noreply@example.com>
To: carol@example.com
Subject: Welcome
...
[ocre mail] end
[ocre jobs] mail done
```

Each `deliver_later` is one queue message: 3 of the 10,000 daily Queues operations on the free plan (September 2026), plus the provider's own limits. Prefer it for mail sent while a user waits, once the app has a job queue; use `send` otherwise.

## Receive email

Cloudflare [Email Routing](https://developers.cloudflare.com/email-service/get-started/route-emails/) hands email sent to addresses of your domain to the Worker. Receiving is free and unlimited on every plan (September 2026). Each email is one run of the Worker's `email` event, with the same CPU limit as a request (10 ms on the free plan): keep the mailbox to a few queries, and enqueue a job for slow work.

```sh
ocre g mailbox
```

```text
  create  src/mailbox.rs
  update  src/lib.rs

Next:
  ocre dev
  curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=support@example.com' --data-binary @message.eml
  route addresses to the Worker: Cloudflare dashboard > Email Routing > Routing rules > Send to a Worker
```

The generator adds this entry point to `src/lib.rs`; `ocre::mail::receive` parses the message, logs `[ocre mail] received from <from> to <to>: <subject>`, and calls `mailbox::receive(ctx, email)` in `src/mailbox.rs`:

```rust
/// Incoming email from Cloudflare Email Routing, handled in src/mailbox.rs.
#[worker::event(email)]
async fn email(message: worker::ForwardableEmailMessage, env: worker::Env, _ctx: worker::Context) -> worker::Result<()> {
    ocre::mail::receive(message, env, mailbox::receive).await
}
```

The handler gets an `ocre::mail::InboundEmail`:

| Method | Returns |
|---|---|
| `email.from()` | Envelope sender (SMTP `MAIL FROM`), checked by Cloudflare; the `From` header may differ |
| `email.to()` | Envelope recipient: which of your addresses received it |
| `email.subject()` | Decoded `Subject`, `""` when missing |
| `email.header(name)` | First header with that name (case-insensitive), decoded |
| `email.headers()` | Every `(name, value)` pair, in order |
| `email.text()` / `email.html()` | First `text/plain` / `text/html` part, decoded to UTF-8 (multipart, quoted-printable, base64, RFC 2047 headers) |
| `email.raw()` | The whole message as received; attachments are only here |
| `email.reject(reason)` | Bounces it: the sender gets a permanent SMTP error with `reason` |
| `email.forward(address).await?` | Forwards it unchanged to a verified destination address |

Returning an `Err` logs `[ocre mail] the mailbox failed: ...` and bounces the email with "The message could not be processed", so the sender knows it was not handled. `email.html()` is the sender's HTML: never render it unescaped.

This mailbox lets one editor post to the blog by email, forwards support mail to a person, and bounces the rest. It uses the blog starter's `Post` model; replace the fixture's `src/mailbox.rs` with it:

```rust,check
// src/mailbox.rs
use ocre::{Ctx, Result, mail::InboundEmail};

use crate::models::post::{self, NewPost};

/// Called once per email that Email Routing sends to the Worker.
pub async fn receive(ctx: Ctx, email: InboundEmail) -> Result<()> {
    match email.to() {
        // Post by email: the subject is the title, the text part the body.
        "posts@example.com" => {
            if email.from() != "ada@example.com" {
                email.reject("Only the editor can post by email");
                return Ok(());
            }
            let body = email.text().unwrap_or_default().trim().to_owned();
            let new = NewPost { title: email.subject().to_owned(), body, published: false };
            let created = post::create(&ctx, new).await?;
            worker::console_log!("mailbox: created post {} from {}", created.id, email.from());
            Ok(())
        }
        // Pass support mail on to a person (a verified destination address).
        "support@example.com" => email.forward("team@example.com").await,
        _ => {
            email.reject("Unknown address");
            Ok(())
        }
    }
}
```

`email.from()` is only as trustworthy as the sending server; do not grant access on the sender address alone. For anything sensitive, also use a hard-to-guess receiving address or a secret in the subject.

### Test it locally

While `ocre dev` runs, POST a raw message to wrangler's local email endpoint; `from` and `to` in the query string are the envelope. The message needs a `Message-ID` header:

```sh
curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=posts@example.com' \
  --data-binary $'From: Ada <ada@example.com>\r\nTo: posts@example.com\r\nSubject: Notes from the road\r\nMessage-ID: <1@example.com>\r\n\r\nWritten on a train.'
curl 'http://localhost:8787/cdn-cgi/local/email?from=mallory@example.com&to=posts@example.com' \
  --data-binary $'From: mallory@example.com\r\nTo: posts@example.com\r\nSubject: Spam\r\nMessage-ID: <2@example.com>\r\n\r\nBuy now'
curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=support@example.com' \
  --data-binary $'From: ada@example.com\r\nTo: support@example.com\r\nSubject: Help\r\nMessage-ID: <3@example.com>\r\n\r\nHello'
curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=posts@example.com' \
  --data-binary $'From: ada@example.com\r\nTo: posts@example.com\r\nSubject: \r\nMessage-ID: <4@example.com>\r\n\r\nNo title'
curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=posts@example.com' \
  --data-binary $'From: ada@example.com\r\nTo: posts@example.com\r\nSubject: No id\r\n\r\nHi'
```

The answers (HTTP 200 for the processed ones, 400 otherwise):

```text
Worker successfully processed email
Worker rejected email with the following reason: Only the editor can post by email
Worker successfully processed email
Worker rejected email with the following reason: The message could not be processed
Email could not be parsed: invalid or no message id provided
```

And the `ocre dev` output:

```text
[ocre mail] received from ada@example.com to posts@example.com: Notes from the road
mailbox: created post 1 from ada@example.com
[ocre mail] received from mallory@example.com to posts@example.com: Spam
[wrangler:error] Email handler rejected message with the following reason: "Only the editor can post by email"
[ocre mail] received from ada@example.com to support@example.com: Help
[wrangler:info] Email handler forwarded message with
  rcptTo: team@example.com
[ocre mail] received from ada@example.com to posts@example.com: 
✘ [ERROR] [ocre mail] the mailbox failed: invalid: Title can't be blank
...
[wrangler:error] Email handler rejected message with the following reason: "The message could not be processed"
```

The email with an empty subject failed the `Post` validation; the `Err` bounced it. Locally, `forward` accepts any address; in production it fails unless the address is a verified destination (the log then says `Fix: add team@example.com as a verified destination address in Cloudflare Email Routing`). To test with a file, save a message as `message.eml` (CRLF line endings) and use `--data-binary @message.eml`.

### Set up Email Routing

Receiving needs a domain on Cloudflare (its DNS managed by Cloudflare); `*.workers.dev` cannot receive email.

1. Deploy the app with the mailbox: `ocre deploy`.
2. In the Cloudflare dashboard, enable Email Routing for the domain; Cloudflare adds the MX and TXT records it needs.
3. For forwarding, add each target as a destination address and confirm it from the email Cloudflare sends.
4. Create a [routing rule](https://developers.cloudflare.com/email-service/configuration/email-routing-addresses/) for an address (for example `posts@yourdomain.com`) with the action "Send to a Worker", and pick the app's Worker. Each rule maps one address to one Worker; a catch-all rule can send every address to it, and `email.to()` tells them apart.

Nothing is added to `wrangler.toml` for receiving.

## See also

- [Background jobs and schedules](jobs.md): the queue behind `deliver_later`, sending mail from jobs.
- [Authentication](authentication.md): magic links and password resets use `ocre::mail::send`.
- [Configuration](../reference/configuration.md): `MAIL_ADAPTER`, `MAIL_FROM`, `RESEND_API_KEY`, the `EMAIL` binding.
- [`ocre g mailer`](../reference/generators.md#ocre-g-mailer) and [`ocre g mailbox`](../reference/generators.md#ocre-g-mailbox).
- [Free-plan limits](../reference/limits.md).
- Rust API: [`ocre::mail`](/api/ocre/mail/index.html), or the [API index](../api-index.md).
