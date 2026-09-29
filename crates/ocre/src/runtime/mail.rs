use std::time::Duration;

use wasm_bindgen::{JsCast, JsValue};
use worker::{
    EmailAddress, EmailAttachment, Env, Fetch, ForwardableEmailMessage, Headers, Method, Request, RequestInit,
    SendEmailBuilder, send::SendFuture,
};

use super::Ctx;
use crate::{
    Error, Result,
    jobs::{DEFAULT_QUEUE, Payload},
    mail::{
        Adapter, Attachment, EMAIL_BINDING, Email, LOG_PREFIX, MAIL_ADAPTER, MAIL_FROM, Message, Outgoing,
        RESEND_API_KEY, RESEND_URL, adapter, cloudflare_error, resend_error, resend_key,
    },
};

/// Sends `email` now, from `MAIL_FROM`, with the adapter named by `MAIL_ADAPTER`.
///
/// `log` prints the whole email to the Worker console between
/// [`LOG_PREFIX`](crate::mail::LOG_PREFIX) lines and sends nothing; `resend`
/// makes one `POST https://api.resend.com/emails` subrequest; `cloudflare`
/// calls the [`EMAIL_BINDING`](crate::mail::EMAIL_BINDING) send_email
/// binding. The request waits for the provider; use
/// [`deliver_later`](crate::mail::deliver_later) to answer first and retry failures.
///
/// The returned future is `Send`, so axum handlers can await it.
///
/// Free-plan limits (September 2026): Resend sends 100 emails a day and 3,000
/// a month from one domain, to any recipient; Cloudflare Email Service on
/// Workers Free only delivers to verified destination addresses of the account.
///
/// # Errors
///
/// - [`Error::BadRequest`](crate::Error::BadRequest) (400): a recipient (`to`, `cc`, `bcc`) or `reply_to` is not an email address.
/// - [`Error::Internal`](crate::Error::Internal) (500), with a log message naming the fix:
///   `MAIL_ADAPTER` unset or unknown; `MAIL_FROM` unset or not an address; a
///   subject that is empty or spans several lines; the `RESEND_API_KEY`
///   secret or the `[[send_email]]` binding named `EMAIL` missing; the
///   provider refused the email (e.g. Resend's quota reached, an unverified
///   domain or recipient) or could not be reached.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::State;
/// use ocre::mail::Email;
/// use ocre::{Ctx, Result};
///
/// async fn invite(State(ctx): State<Ctx>) -> Result<&'static str> {
///     let email = Email::new("ada@example.com", "You're invited", "Join us: https://example.com/join");
///     ocre::mail::send(&ctx, email).await?;
///     Ok("invited")
/// }
/// ```
pub fn send(ctx: &Ctx, email: Email) -> impl Future<Output = Result<()>> + Send + use<> {
    let env = ctx.env().clone();
    SendFuture::new(async move { deliver(&env, email).await })
}

/// Checks `email` now and sends it later from the background jobs queue, like Rails' `deliver_later`.
///
/// The address, `MAIL_FROM` and `MAIL_ADAPTER` are checked right away, like
/// [`send`](crate::mail::send), so a bad address is still a 400 for the
/// request. The email is then put on the `JOBS` queue and the handler answers
/// without waiting for the provider; [`consume`](crate::jobs::consume) sends
/// it with [`send`](crate::mail::send), logs `[ocre jobs] mail done`, and on
/// failure retries it with the jobs backoff (30 s, 1 min, 3 min, 9 min, 27
/// min) before the dead-letter queue. The Resend key and the send_email
/// binding are only looked up when the consumer sends.
///
/// Needs the `JOBS` queue: run `ocre g job <Name>` once to wire it.
///
/// Free-plan cost: one queue message, i.e. 3 of the 10,000 daily Queues
/// operations (write, read, delete), one more read per retry and one more
/// write if it is dead-lettered; plus the provider's limits of
/// [`send`](crate::mail::send).
///
/// # Errors
///
/// - [`Error::BadRequest`](crate::Error::BadRequest) (400): a recipient or `reply_to` is not an email address.
/// - [`Error::Internal`](crate::Error::Internal) (500): `MAIL_ADAPTER` unset or
///   unknown, `MAIL_FROM` unset or not an address, a subject that is empty or
///   spans several lines, an email over the 128 KB queue message limit, the
///   `[[queues.producers]]` binding named `JOBS` missing from wrangler.toml, or
///   Queues refusing the message.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::State;
/// use ocre::mail::Email;
/// use ocre::{Ctx, Result};
///
/// async fn sign_up(State(ctx): State<Ctx>) -> Result<&'static str> {
///     let email = Email::new("ada@example.com", "Welcome", "Hello Ada");
///     ocre::mail::deliver_later(&ctx, email).await?;
///     Ok("check your inbox")
/// }
/// ```
pub fn deliver_later(ctx: &Ctx, email: Email) -> impl Future<Output = Result<()>> + Send + use<> {
    deliver_in(ctx, email, Duration::ZERO)
}

/// Checks `email` now and sends it from the jobs queue after `delay`, like Rails' `deliver_later(wait:)`.
///
/// Works like [`deliver_later`](crate::mail::deliver_later), with the
/// message due after `delay` (whole seconds, 24 hours at most, Queues'
/// limit): a reminder an hour after sign-up, a digest at the end of the
/// day. Same free-plan cost as [`deliver_later`](crate::mail::deliver_later).
///
/// # Errors
///
/// Those of [`deliver_later`](crate::mail::deliver_later), plus an
/// [`Error::Internal`](crate::Error::Internal) when `delay` is over 24 hours.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use ocre::mail::Email;
///
/// async fn remind(ctx: &ocre::Ctx) -> ocre::Result<()> {
///     let email = Email::new("ada@example.com", "Finish setting up your shop", "https://example.com/setup");
///     ocre::mail::deliver_in(ctx, email, Duration::from_secs(3600)).await
/// }
/// ```
pub fn deliver_in(ctx: &Ctx, email: Email, delay: Duration) -> impl Future<Output = Result<()>> + Send + use<> {
    let env = ctx.env();
    let checked = adapter(var(env, MAIL_ADAPTER).as_deref())
        .and_then(|_| Outgoing::new(var(env, MAIL_FROM), email))
        .map(|outgoing| Payload::Mail(Box::new(outgoing.email)));
    super::jobs::send(env.clone(), DEFAULT_QUEUE, checked, delay)
}

fn var(env: &Env, name: &str) -> Option<String> {
    env.var(name).ok().map(|value| value.to_string())
}

pub(crate) async fn deliver(env: &Env, email: Email) -> Result<()> {
    let adapter = adapter(var(env, MAIL_ADAPTER).as_deref())?;
    let outgoing = Outgoing::new(var(env, MAIL_FROM), email)?;
    match adapter {
        Adapter::Log => {
            worker::console_log!("{}", outgoing.log_text());
            crate::mail::capture(outgoing);
            Ok(())
        }
        Adapter::Resend => resend(env, &outgoing).await,
        Adapter::Cloudflare => cloudflare(env, &outgoing).await,
    }
}

async fn resend(env: &Env, outgoing: &Outgoing) -> Result<()> {
    let key = resend_key(env.secret(RESEND_API_KEY).ok().map(|secret| secret.to_string()))?;
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {key}"))?;
    headers.set("Content-Type", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&outgoing.resend_json().to_string())));
    let mut response = Fetch::Request(Request::new_with_init(RESEND_URL, &init)?).send().await?;
    let status = response.status_code();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(resend_error(status, &response.text().await.unwrap_or_default()))
    }
}

async fn cloudflare(env: &Env, outgoing: &Outgoing) -> Result<()> {
    let binding = env.send_email(EMAIL_BINDING).map_err(|err| {
        Error::internal(format!(
            "cannot send email: the send_email binding `{EMAIL_BINDING}` is missing ({err}). Fix: add a \
             [[send_email]] entry with name = \"{EMAIL_BINDING}\" to wrangler.toml"
        ))
    })?;
    let Outgoing { from, email } = outgoing;
    let to = Outgoing::addresses(&email.to);
    let builder = match &from.name {
        Some(name) => SendEmailBuilder::builder_with_email_address_and_slice(
            &EmailAddress::new(name, &from.address),
            &to,
            &email.subject,
        ),
        None => SendEmailBuilder::builder_with_str_and_slice(&from.address, &to, &email.subject),
    };
    let mut builder = builder.text(&email.text);
    if let Some(html) = &email.html {
        builder = builder.html(html);
    }
    if let Some(reply_to) = &email.reply_to {
        builder = builder.reply_to(reply_to);
    }
    if !email.cc.is_empty() {
        builder = builder.cc_with_slice(&Outgoing::addresses(&email.cc));
    }
    if !email.bcc.is_empty() {
        builder = builder.bcc_with_slice(&Outgoing::addresses(&email.bcc));
    }
    if !email.headers.is_empty() {
        let headers = worker::js_sys::Object::new();
        for (name, value) in &email.headers {
            // Setting a property on a plain object cannot fail.
            let _ = worker::js_sys::Reflect::set(&headers, &JsValue::from_str(name), &JsValue::from_str(value));
        }
        builder = builder.headers(headers.unchecked_ref());
    }
    if !email.attachments.is_empty() {
        let files: Vec<EmailAttachment> = email
            .attachments
            .iter()
            .map(|file| {
                let bytes = worker::js_sys::Uint8Array::from(file.content.as_slice());
                match &file.content_id {
                    Some(id) => {
                        EmailAttachment::new_inline_with_typed_array(id, &file.filename, &file.content_type, &bytes)
                    }
                    None => {
                        EmailAttachment::new_attachment_with_typed_array(&file.filename, &file.content_type, &bytes)
                    }
                }
            })
            .collect();
        builder = builder.attachments(&files);
    }
    binding.send_with_builder(&builder.build()).await.map_err(|err| cloudflare_error(&js_error(&err)))?;
    Ok(())
}

/// `message` of a JS error, plus its `code` (`E_SENDER_NOT_VERIFIED`, ...) when set.
fn js_error(err: &worker::js_sys::Error) -> String {
    let message = String::from(err.message());
    match worker::js_sys::Reflect::get(err, &JsValue::from_str("code")).ok().and_then(|code| code.as_string()) {
        Some(code) => format!("{code}: {message}"),
        None => message,
    }
}

/// An email that Cloudflare Email Routing delivered to the Worker, handed to the app's mailbox.
///
/// [`receive`](crate::mail::receive) builds it. The envelope ([`from`](Self::from), [`to`](Self::to)) comes from
/// Cloudflare; headers and bodies are parsed from the raw message
/// (multipart, quoted-printable, base64, RFC 2047 encoded words; files in
/// [`attachments`](Self::attachments), the bytes in [`raw`](Self::raw)). Bounce it with
/// [`reject`](Self::reject) or pass it on with [`forward`](Self::forward).
/// Receiving is free and unlimited on every plan.
///
/// # Examples
///
/// ```no_run
/// use ocre::mail::InboundEmail;
/// use ocre::{Ctx, Result};
///
/// // src/mailbox.rs
/// pub async fn receive(_ctx: Ctx, email: InboundEmail) -> Result<()> {
///     match email.to() {
///         "support@example.com" => email.forward("team@example.com").await?,
///         _ => email.reject("Unknown address"),
///     }
///     Ok(())
/// }
/// ```
pub struct InboundEmail {
    inner: ForwardableEmailMessage,
    from: String,
    to: String,
    raw: Vec<u8>,
    message: Message,
}

impl InboundEmail {
    /// Returns the envelope sender (SMTP `MAIL FROM`), checked by Cloudflare.
    ///
    /// The `From` header may differ: read it with `email.header("From")`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// if email.from().ends_with("@example.com") {
    ///     // a colleague
    /// }
    /// # }
    /// ```
    pub fn from(&self) -> &str {
        &self.from
    }

    /// Returns the envelope recipient: the address of this app that received the email.
    ///
    /// Match on it to route several addresses to one Worker.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// let team = email.to().starts_with("support@");
    /// # let _ = team;
    /// # }
    /// ```
    pub fn to(&self) -> &str {
        &self.to
    }

    /// Returns the decoded `Subject` header, or `""` when it is missing.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// let urgent = email.subject().contains("URGENT");
    /// # let _ = urgent;
    /// # }
    /// ```
    pub fn subject(&self) -> &str {
        self.message.header("Subject").unwrap_or("")
    }

    /// Returns the first header with this name (case-insensitive), decoded.
    ///
    /// Folded lines are joined and RFC 2047 encoded words decoded. `None`
    /// when the message has no such header.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// let id = email.header("message-id").unwrap_or("");
    /// # let _ = id;
    /// # }
    /// ```
    pub fn header(&self, name: &str) -> Option<&str> {
        self.message.header(name)
    }

    /// Returns every header, in message order, as decoded `(name, value)` pairs.
    ///
    /// Repeated headers (`Received`, ...) appear once per occurrence.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// let hops = email.headers().iter().filter(|(name, _)| name.eq_ignore_ascii_case("Received")).count();
    /// # let _ = hops;
    /// # }
    /// ```
    pub fn headers(&self) -> &[(String, String)] {
        &self.message.headers
    }

    /// Returns the first `text/plain` part, decoded to UTF-8, or `None` when there is none.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// let body = email.text().unwrap_or_default();
    /// # let _ = body;
    /// # }
    /// ```
    pub fn text(&self) -> Option<&str> {
        self.message.text.as_deref()
    }

    /// Returns the files of the email, decoded: every part that is not the first text or HTML body.
    ///
    /// Inline images carry their `content_id`. Store them in R2
    /// (`ocre::storage`) rather than D1; they are the sender's files, so
    /// check the type and size before keeping them.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// for file in email.attachments() {
    ///     worker::console_log!("{} ({}, {} bytes)", file.filename, file.content_type, file.content.len());
    /// }
    /// # }
    /// ```
    pub fn attachments(&self) -> &[Attachment] {
        &self.message.attachments
    }

    /// Returns the first `text/html` part, decoded to UTF-8, or `None` when there is none.
    ///
    /// It is the sender's HTML: never render it unescaped.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// let has_html = email.html().is_some();
    /// # let _ = has_html;
    /// # }
    /// ```
    pub fn html(&self) -> Option<&str> {
        self.message.html.as_deref()
    }

    /// Returns the whole message as received (RFC 5322 bytes), e.g. to store it in R2 or read attachments.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// let size = email.raw().len();
    /// # let _ = size;
    /// # }
    /// ```
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// Bounces the email: the sending server gets a permanent SMTP error with `reason`.
    ///
    /// The mailbox handler still returns normally; [`receive`](crate::mail::receive)
    /// also bounces the email when the handler returns an `Err`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) {
    /// if email.to() != "support@example.com" {
    ///     email.reject("Unknown address");
    /// }
    /// # }
    /// ```
    pub fn reject(&self, reason: &str) {
        self.inner.set_reject(reason);
    }

    /// Forwards the email unchanged to `to`, which must be a verified destination address of the Cloudflare account.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) when Cloudflare refuses, typically because
    /// `to` is not a verified destination address in Email Routing (the
    /// message says so).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn mailbox(email: ocre::mail::InboundEmail) -> ocre::Result<()> {
    /// email.forward("team@example.com").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn forward(&self, to: &str) -> Result<()> {
        self.inner.forward(to).await.map_err(|err| {
            Error::internal(format!(
                "forwarding the email to {to} failed ({}). Fix: add {to} as a verified destination address \
                 in Cloudflare Email Routing",
                js_error(&err)
            ))
        })?;
        Ok(())
    }
}

/// Runs the app's mailbox `handler` for an email from Cloudflare Email Routing; the Worker's `email` entry point.
///
/// Reads the raw message, parses it into an [`InboundEmail`], logs one
/// `[ocre mail] received from <from> to <to>: <subject>` line and calls
/// `handler(ctx, email)` with a fresh [`Ctx`](crate::Ctx). When the handler
/// returns an `Err`, the error is logged (`[ocre mail] the mailbox failed:
/// ...`) and the email bounced ("The message could not be processed"), so the
/// sender knows it was not handled. Routing rules in the dashboard (Email
/// Routing > Routing rules) send an address to the Worker; receiving is free
/// and unlimited on every plan.
///
/// # Errors
///
/// A `worker::Error` only when the raw message cannot be read; handler
/// errors are logged and bounce the email instead.
///
/// # Examples
///
/// `ocre g mailbox` writes the entry point in `src/lib.rs` and the handler in `src/mailbox.rs`:
///
/// ```no_run
/// mod mailbox {
///     use ocre::{Ctx, Result, mail::InboundEmail};
///
///     pub async fn receive(_ctx: Ctx, email: InboundEmail) -> Result<()> {
///         email.forward("team@example.com").await
///     }
/// }
///
/// #[worker::event(email)]
/// async fn email(
///     message: worker::ForwardableEmailMessage,
///     env: worker::Env,
///     _ctx: worker::Context,
/// ) -> worker::Result<()> {
///     ocre::mail::receive(message, env, mailbox::receive).await
/// }
/// # fn main() {}
/// ```
pub async fn receive<F, Fut>(message: ForwardableEmailMessage, env: Env, handler: F) -> worker::Result<()>
where
    F: FnOnce(Ctx, InboundEmail) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let raw = message.raw_bytes().await?;
    let parsed = Message::parse(&raw);
    let email = InboundEmail { from: message.from(), to: message.to(), raw, message: parsed, inner: message.clone() };
    worker::console_log!("{LOG_PREFIX} received from {} to {}: {}", email.from(), email.to(), email.subject());
    if let Err(err) = handler(Ctx::new(env), email).await {
        worker::console_error!("{LOG_PREFIX} the mailbox failed: {err}");
        message.set_reject("The message could not be processed");
    }
    Ok(())
}
