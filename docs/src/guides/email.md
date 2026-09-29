# Email

This guide sends email from an Ocre app with `ocre::mail` (built directly, or by a mailer generated with `ocre g mailer`, with layouts, app-wide defaults and previews), adds several recipients, headers, attachments and inline images, picks a delivery adapter for development and production, sends from the background with `deliver_later`, inspects every email in `ocre dev`, and receives email through Cloudflare Email Routing with `ocre g mailbox`.

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
| `.also_to(address)`, `.cc(address)`, `.bcc(address)` | More recipients; `To` and `Cc` see each other, `Bcc` recipients are hidden. 50 recipients at most (`to`, `cc` and `bcc` together) |
| `.html(html)` | Adds an HTML body, shown by clients that render HTML. Sent as given: escape user input (askama templates do) |
| `.reply_to(address)` | Replies go there instead of to the sender |
| `.from(address)` | Sends from this address instead of `MAIL_FROM` (on a domain verified with the provider) |
| `.header(name, value)` | An extra header: `In-Reply-To` and `References` to thread a reply, `List-Unsubscribe`, `X-...` |
| `.attach(filename, content_type, bytes)` | Attaches a file (see [Attachments and inline images](#attachments-and-inline-images)) |
| `.inline(content_id, filename, content_type, bytes)` | An image the HTML shows with `<img src="cid:content_id">` |
| Fields `from`, `to`, `cc`, `bcc`, `subject`, `text`, `html`, `reply_to`, `headers`, `attachments` | Public, for reading or changing an email a mailer built |

Every address may carry a display name, `Ada Lovelace <ada@example.com>`. `ocre::mail::address_with_name("Acme, Inc.", "x@acme.test")` builds one from a user-typed name, quoting it when needed (`"Acme, Inc." <x@acme.test>`) and removing characters that would break the header, like Rails' `email_address_with_name`.

Nothing is checked while building. `send` checks, then delivers:

| Problem | Result |
|---|---|
| A recipient (`to`, `cc`, `bcc`) or `reply_to` is not an email address | `Error::BadRequest`, 400 `invalid email address: <address>` |
| `MAIL_ADAPTER` unset or unknown, the sender unset or not an address, no recipient or more than 50, subject empty or on several lines, a header Ocre sets itself (`Subject`, `Cc`...) or with a line break, an attachment without a name or a MIME type, `RESEND_API_KEY` or the `EMAIL` binding missing, the provider refused or could not be reached | `Error::Internal`, 500; the log line says what to fix |

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
| `log` | Prints the whole email (headers, text, HTML, one line per attachment) to the Worker console between `[ocre mail]` lines; sends nothing. In `ocre dev` it also keeps the last 20 for the [development pages](#preview-and-inspect-emails-in-development) | none; `ocre new` writes `MAIL_ADAPTER=log` to `.dev.vars`, which overrides `[vars]` in `ocre dev` | none |
| `resend` | `POST https://api.resend.com/emails` | `RESEND_API_KEY` secret; `MAIL_FROM` on a domain verified in Resend | [Resend free plan](https://resend.com/docs/knowledge-base/account-quotas-and-limits): 100 emails a day, 3,000 a month, one domain; any recipient |
| `cloudflare` | Cloudflare Email Service through the `EMAIL` [send_email binding](https://developers.cloudflare.com/email-service/api/send-emails/workers-api/) | `[[send_email]] name = "EMAIL"` in `wrangler.toml`; `MAIL_FROM` on a domain onboarded to Email Service | [Workers Free](https://developers.cloudflare.com/email-service/platform/pricing/): only verified destination addresses of the account; any recipient needs Workers Paid (3,000 a month included, then $0.35 per 1,000) |

For sign-up, magic-link and password-reset mail to any address on the free plan, use Resend. The `cloudflare` adapter on the free plan suits mail to yourself (alerts, reports).

With `MAIL_ADAPTER` unset, `send` fails rather than dropping mail silently. With `MAIL_ADAPTER` removed from `.dev.vars`, a handler that sends answers 500 and `ocre dev` logs:

```text
✘ [ERROR] [ocre] cannot send email: MAIL_ADAPTER is not set. Fix: set MAIL_ADAPTER to "resend" (with the RESEND_API_KEY secret) or "cloudflare" (with a [[send_email]] binding named EMAIL) under [vars] in wrangler.toml; `ocre new` puts MAIL_ADAPTER=log in .dev.vars so `ocre dev` only logs mail
```

### log (development)

This is what `ocre dev` prints for an email with a text and an HTML body (a generated mailer's, so the HTML comes wrapped in `templates/mailers/layout.html`):

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
<head>
...
<p>Hello bob@example.com,</p>
<p>This is the welcome email. Edit templates/mailers/user/welcome.html.</p>

</body>
</html>
[ocre mail] end
```

`Cc`, `Bcc`, `Reply-To` and extra headers are printed with the others, and each attachment as a line such as `[ocre mail] attachment: digest.csv (text/csv, 4 bytes)`. Links in emails (magic links, password resets) can be opened from there, or from the [development pages](#preview-and-inspect-emails-in-development). In production, `log` writes to the Worker logs (`npx wrangler tail`); it never delivers.

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
  create  templates/mailers/layout.html
  create  templates/mailers/layout.txt
  create  src/mailers/mod.rs
  update  src/lib.rs

Next:
  send it from a handler: ocre::mail::send(&ctx, mailers::user::welcome(&address)?).await?
  ocre dev, then open http://localhost:8787/ocre/dev/mailers to preview it (MAIL_ADAPTER=log in .dev.vars prints each email sent instead of sending it)
```

`src/mailers/user.rs` has one function per action. Each renders a `.txt` and an `.html` template with askama, and passes the email through the app's `defaults`:

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
    Ok(super::defaults(Email::new(to, "Welcome", text).html(html)))
}
```

```text
{% extends "mailers/layout.txt" %}
{% block content -%}
Hello {{ to }},

This is the welcome email. Edit templates/mailers/user/welcome.txt.
{%- endblock %}
```

The subject is the humanized action name ("Welcome", "Password reset"); change it in the function, or translate it (see [I18n](i18n.md): `LOCALES.locale(locale).t("mailers.user.welcome.subject")`, Rails' `default_i18n_subject`). `.txt` templates are not HTML-escaped (they are plain text); `.html` templates are. The template of an action is the `path` of its struct: point it anywhere under `templates/` (Rails' `template_path` and `template_name`). In an API-only app (`ocre new --api`), the mailer has no templates: each function builds the text with `format!` and sends no HTML; add `.html(...)` yourself.

### Layouts

Every generated template extends `templates/mailers/layout.html` or `layout.txt` (created by the first mailer), like Rails' `mailer.html.erb`: the HTML layout holds the document, the inline styles mail clients need, and `{% block content %}{% endblock %}` where each email goes. Put a header, a footer or a signature there once. A mailer that needs another layout extends another file (`{% extends "mailers/billing_layout.html" %}`); shared pieces go in partials with `{% include "mailers/_footer.html" %}`.

### Defaults and callbacks: src/mailers/mod.rs

The first mailer creates `src/mailers/mod.rs`, which plays the part of Rails' `ApplicationMailer`:

```rust
/// Applied to every email of the mailers, like Rails' `default from:` and
/// `after_action`: e.g. `email.from("Shop <hello@yourdomain.com>")`,
/// `.bcc("archive@yourdomain.com")` or `.header("List-Unsubscribe", ...)`.
/// Without `from`, emails come from the MAIL_FROM variable.
pub fn defaults(email: Email) -> Email {
    email
}

/// Emails shown at /ocre/dev/mailers in `ocre dev` (debug builds only), built
/// with sample data: change the arguments to realistic values.
pub static PREVIEWS: &[Preview] = &[
    // ocre:mailer-previews
    Preview::new("user/welcome", || user::welcome("ada@example.com")),
    Preview::new("user/password_reset", || user::password_reset("ada@example.com")),
];
```

Rails' other mailer hooks are plain functions too:

- `before_action` and parameterized mailers (`with(params)`): the action's arguments. A mailer function takes what it needs, typed.
- `before_deliver`, `after_deliver`, interceptors and observers: one app function that every handler calls instead of `ocre::mail::send`:

  ```rust
  // src/mailers/mod.rs
  /// Sends through the app's rules: staging mail goes to the team only (an
  /// interceptor), and every delivery is logged (an observer).
  pub async fn deliver(ctx: &ocre::Ctx, mut email: Email) -> ocre::Result<()> {
      if ctx.env().var("STAGING").is_ok() {
          email.subject = format!("[to {}] {}", email.to.join(", "), email.subject);
          email.to = vec!["team@example.com".to_owned()];
          email.cc.clear();
          email.bcc.clear();
      }
      ocre::mail::send(ctx, email.clone()).await?;
      worker::console_log!("delivered {:?} to {:?}", email.subject, email.to);
      Ok(())
  }
  ```

- `rescue_from`: a mailer function returns `Result`; handle its error where you call it. A `deliver_later` email that fails is retried by the queue.

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

and use them in the templates (`Hello {{ name }},` and `{{ account_url }}`). Pass values (strings, numbers), not a database handle: a mailer only builds the email, the caller loads the data. Links and images need absolute URLs, since the email is read outside the site: in a handler, build them from the request (`crate::auth::origin(&uri)` after `ocre g auth`) and pass the result, which replaces Rails' `default_url_options` and `asset_host`; or embed images with [`inline`](#attachments-and-inline-images).

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

`ocre::mail::deliver_in(&ctx, email, delay).await?` does the same with the message due after `delay` (24 hours at most), Rails' `deliver_later(wait:)`: a reminder an hour after sign-up.

Each `deliver_later` is one queue message: 3 of the 10,000 daily Queues operations on the free plan (September 2026), plus the provider's own limits. Prefer it for mail sent while a user waits, once the app has a job queue; use `send` otherwise.

## Attachments and inline images

`.attach(filename, content_type, bytes)` adds a file, like Rails' `attachments["name"] = ...`; `.inline(content_id, filename, content_type, bytes)` adds an image that the HTML shows with `cid:`, like `attachments.inline`. Mail clients show inline images without loading anything remote, so they are the reliable way to put a logo in an email:

```rust,check
use ocre::{Ctx, Result, mail::Email};

/// A monthly report: a CSV attachment and the logo inline (`logo` holds the
/// PNG's bytes, e.g. `include_bytes!("../assets/logo.png").to_vec()`).
pub async fn send_report(ctx: &Ctx, to: &str, csv: String, logo: Vec<u8>) -> Result<()> {
    let email = Email::new(to, "Your monthly report", "The report is attached.")
        .html("<p><img src=\"cid:logo\" alt=\"Shop\"></p><p>The report is attached.</p>")
        .inline("logo", "logo.png", "image/png", logo)
        .attach("report.csv", "text/csv", csv.into_bytes());
    ocre::mail::send(ctx, email).await
}
```

Both adapters send them (Resend as `attachments` with `content_id`, Cloudflare Email Service as attachments with an inline disposition), and `log` lists them. Sizes: Resend accepts 40 MB per email; with `deliver_later` the whole email must fit a 128 KB queue message, and files grow by a third in it (base64). For large files, store them in R2 ([Files](files.md)) and send a link.

## Preview and inspect emails in development

While `ocre dev` runs, the app serves development pages, like Rails' `/rails/mailers` and letter_opener. `ocre g mailer` and `ocre g mailbox` add them to `routes()` in `src/lib.rs`:

```rust
        .merge(ocre::mail::dev_routes(mailers::PREVIEWS))
```

| Page | Shows |
|---|---|
| `/ocre/dev/mailers` | The previews of `PREVIEWS` in `src/mailers/mod.rs`, and the last 20 emails sent with `MAIL_ADAPTER=log` (from requests, jobs and `deliver_later`) |
| `/ocre/dev/mailers/preview/<mailer>/<action>` | One preview, rendered now with its sample data: headers, the HTML in a sandboxed frame (inline images shown), the text, the attachments |
| `/ocre/dev/mailers/sent/<id>` | One email sent |
| `/ocre/dev/mailers/sent.json` | The emails sent, as JSON (`[{"id": 1, "from": "...", "email": {"to": [...], "subject": ..., "text": ...}}]`), for end-to-end tests |
| `/ocre/dev/mailbox` | A form that delivers a test email to the app's mailbox (see [Receive email](#receive-email)) |

`ocre g mailer` adds a preview per action, called with `"ada@example.com"`; change the arguments to realistic data. The pages only exist in debug builds, which is what `ocre dev` builds; the release build of `ocre deploy` compiles them out, so they are 404s in production. They use no billed resource: previews render in the request, sent emails are kept in the Worker's memory (lost when `ocre dev` restarts).

## Test mailers

A mailer is a function that returns an `Email`, so unit tests call it natively, like Rails' `ActionMailer::TestCase`:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn welcome_greets_the_user() {
        let email = super::welcome("ada@example.com").unwrap();
        assert_eq!(email.to, ["ada@example.com"]);
        assert_eq!(email.subject, "Welcome");
        assert!(email.text.contains("Hello ada@example.com"));
    }
}
```

Deliveries (Rails' `assert_emails`) are checked end to end against `ocre dev`: trigger the request, then read `/ocre/dev/mailers/sent.json`, for example to follow a magic link:

```sh
curl -s http://localhost:8787/ocre/dev/mailers/sent.json | jq -r '.[-1].email.text'
```

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
  open http://localhost:8787/ocre/dev/mailbox to deliver a test email
  or: curl 'http://localhost:8787/cdn-cgi/local/email?from=ada@example.com&to=support@example.com' --data-binary @message.eml
  route addresses to the Worker: Cloudflare dashboard > Email Routing > Routing rules > Send to a Worker
```

The generator adds this entry point to `src/lib.rs` (and the [development pages](#preview-and-inspect-emails-in-development) to `routes()`, unless a mailer already did); `ocre::mail::receive` parses the message, logs `[ocre mail] received from <from> to <to>: <subject>`, and calls `mailbox::receive(ctx, email)` in `src/mailbox.rs`:

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
| `email.attachments()` | The files: every part that is not the first text or HTML body, decoded (`filename`, `content_type`, `content`, and `content_id` for inline images) |
| `email.raw()` | The whole message as received (RFC 5322 bytes), e.g. to store in R2 |
| `email.reject(reason)` | Bounces it: the sender gets a permanent SMTP error with `reason` |
| `email.forward(address).await?` | Forwards it unchanged to a verified destination address |

Returning an `Err` logs `[ocre mail] the mailbox failed: ...` and bounces the email with "The message could not be processed", so the sender knows it was not handled. `email.html()` is the sender's HTML: never render it unescaped; attachments are the sender's files: check their type and size before keeping them.

Routing is a `match` on `email.to()`, Rails' mailbox `routing`: string patterns for exact addresses, guards for the rest (`to if to.starts_with("reply+") => ...` for plus-addressed replies, `to if to.ends_with("@support.example.com") => ...`), and `_` for the catch-all. Rails' `before_processing` and `after_processing` are code before and after the `match`; `bounce_with` is `email.reject(reason)`. Cloudflare Email Routing is the only ingress: providers such as Mailgun or Postmark (Rails' other ingresses) would post to an ordinary route instead.

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

While `ocre dev` runs, the simplest way is the form at `http://localhost:8787/ocre/dev/mailbox`, Rails' Action Mailbox conductor: fill in the envelope, the subject and the body, and it delivers the message through wrangler's local email endpoint, showing the answer (`200: Worker successfully processed email`). The same endpoint works from a terminal: POST a raw message; `from` and `to` in the query string are the envelope. The message needs a `Message-ID` header:

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

### Keep a record of inbound email

Rails stores every inbound email with a status (Action Mailbox's `InboundEmail`) and deletes it after a while. In Ocre that is a table and a few lines of the mailbox, for apps that need it (an audit, a support inbox). A migration:

```sql
CREATE TABLE inbound_emails (
  id INTEGER PRIMARY KEY,
  message_id TEXT NOT NULL UNIQUE,
  sender TEXT NOT NULL,
  recipient TEXT NOT NULL,
  subject TEXT NOT NULL,
  status TEXT NOT NULL,          -- processing, delivered, bounced, failed
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

and the mailbox records each email before handling it and its outcome after:

```rust
pub async fn receive(ctx: Ctx, email: InboundEmail) -> Result<()> {
    let db = ctx.db()?;
    let id = email.header("Message-ID").unwrap_or_default().to_owned();
    let fresh = db
        .execute(
            "INSERT INTO inbound_emails (message_id, sender, recipient, subject, status) \
             VALUES (?1, ?2, ?3, ?4, 'processing') ON CONFLICT (message_id) DO NOTHING",
            params![id.clone(), email.from(), email.to(), email.subject()],
        )
        .await?;
    if fresh == 0 {
        return Ok(()); // a redelivery of an email already handled
    }
    let result = route(&ctx, &email).await; // the `match email.to()` above, returning the status
    let status = match &result {
        Ok(status) => *status, // "delivered" or "bounced"
        Err(_) => "failed",
    };
    db.execute("UPDATE inbound_emails SET status = ?1 WHERE message_id = ?2", params![status, id]).await?;
    result.map(|_| ())
}
```

Each email costs 2 D1 rows written (100,000 a day on the free plan). Keep the raw message in R2 if you need it later (`email.raw()`; D1 rows hold 2 MB at most). Rails' incineration is a [schedule](jobs.md#schedules) that deletes old rows: `ocre g schedule incinerate_inbound_emails "every day at 4am"` with `DELETE FROM inbound_emails WHERE created_at < datetime('now', '-30 days')`.

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
