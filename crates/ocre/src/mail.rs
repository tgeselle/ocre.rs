//! Email: send with adapters (log, Resend, Cloudflare), receive from Email Routing.
//!
//! Sending is Rails' Action Mailer without the class: build an [`Email`] and
//! hand it to [`send`], or to
//! [`deliver_later`] to send it from the
//! background jobs queue (see [`jobs`](crate::jobs)) so the request does not
//! wait for the provider and a failed delivery is retried. Receiving is
//! Action Mailbox: [`receive`] is the Worker's `email`
//! entry point and hands each message to the app as an [`InboundEmail`].
//!
//! ```no_run
//! use axum::extract::State;
//! use ocre::mail::{self, Email};
//! use ocre::{Ctx, Result};
//!
//! async fn welcome(State(ctx): State<Ctx>) -> Result<&'static str> {
//!     let email = Email::new("ada@example.com", "Welcome", "Hello Ada,\n\nhttps://example.com/start\n")
//!         .html("<p>Hello Ada,</p><p><a href=\"https://example.com/start\">Start</a></p>");
//!     mail::send(&ctx, email).await?;
//!     Ok("sent")
//! }
//! ```
//!
//! # Configuration
//!
//! The [`MAIL_ADAPTER`] Worker variable picks how
//! mail leaves the Worker, with no guessing from which keys happen to be set,
//! so a development machine holding a real API key never sends by accident:
//!
//! - `log`: prints the whole email (headers, text, HTML) to the Worker console
//!   between [`LOG_PREFIX`] lines, like Rails'
//!   letter_opener. `ocre new` writes `MAIL_ADAPTER=log` to `.dev.vars`, which
//!   overrides `[vars]` in `ocre dev`. No configuration, no limits.
//! - `resend`: `POST https://api.resend.com/emails` with the
//!   [`RESEND_API_KEY`] secret, `MAIL_FROM` on a
//!   domain verified in Resend. Free plan (September 2026): 100 emails a day,
//!   3,000 a month, one domain; any recipient.
//! - `cloudflare`: Cloudflare Email Service through the
//!   [`EMAIL_BINDING`] `[[send_email]]` binding,
//!   `MAIL_FROM` on a domain onboarded to Email Service. Workers Free
//!   (September 2026) only delivers to verified destination addresses of the
//!   account; any recipient needs Workers Paid (3,000 a month included).
//!
//! The sender is the [`MAIL_FROM`] variable,
//! `noreply@example.com` or `Name <noreply@example.com>`. With `MAIL_ADAPTER`
//! unset, sending fails with an [`Error::Internal`]
//! naming the fix, so a production Worker never drops mail silently. For
//! sign-up and password-reset mail on the free plan, use Resend.
//!
//! Receiving uses Cloudflare Email Routing, free and unlimited on every plan.
//! Every line Ocre logs about mail starts with [`LOG_PREFIX`].

mod parse;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use crate::runtime::mail::{InboundEmail, deliver_later, receive, send};
pub(crate) use parse::Message;

use crate::{Error, Result, validate::is_email};

/// Name of the Worker variable that chooses the adapter: `log`, `resend` or `cloudflare`.
///
/// Read on every [`send`] and [`deliver_later`]; unset or any other value is
/// an [`Error::Internal`] that names the fix. Set it under
/// `[vars]` in wrangler.toml, or in `.dev.vars` for `ocre dev` (which overrides `[vars]`).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::mail::MAIL_ADAPTER, "MAIL_ADAPTER");
/// ```
pub const MAIL_ADAPTER: &str = "MAIL_ADAPTER";
/// Name of the Worker variable holding the sender address.
///
/// Either `noreply@example.com` or `Name <noreply@example.com>` (the name
/// may be quoted). Missing or unparsable is an
/// [`Error::Internal`] when sending. With Resend or
/// Cloudflare the domain must be verified with that provider.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::mail::MAIL_FROM, "MAIL_FROM");
/// ```
pub const MAIL_FROM: &str = "MAIL_FROM";
/// Name of the Worker secret holding the Resend API key, used when `MAIL_ADAPTER = "resend"`.
///
/// Set it with `npx wrangler secret put RESEND_API_KEY` (and in `.dev.vars`
/// to send for real from `ocre dev`). Missing or blank is an
/// [`Error::Internal`] when sending.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::mail::RESEND_API_KEY, "RESEND_API_KEY");
/// ```
pub const RESEND_API_KEY: &str = "RESEND_API_KEY";
/// Name of the `[[send_email]]` binding used when `MAIL_ADAPTER = "cloudflare"`.
///
/// `ocre new` leaves the entry commented out in wrangler.toml; a missing
/// binding is an [`Error::Internal`] naming the entry to add.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::mail::EMAIL_BINDING, "EMAIL");
/// ```
pub const EMAIL_BINDING: &str = "EMAIL";
/// Prefix of every line Ocre logs about mail, e.g. in `ocre dev` output.
///
/// The `log` adapter frames each email between lines with this prefix, and
/// [`receive`] logs `[ocre mail] received from ... to ...: <subject>`.
///
/// # Examples
///
/// ```
/// assert!("[ocre mail] end".starts_with(ocre::mail::LOG_PREFIX));
/// ```
pub const LOG_PREFIX: &str = "[ocre mail]";

/// An outgoing email: one recipient, a subject, a plain-text body and an optional HTML body.
///
/// Build it with [`Email::new`], then add an HTML version with
/// [`html`](Self::html) and a `Reply-To` with [`reply_to`](Self::reply_to);
/// send it with [`send`] or [`deliver_later`]. Nothing is checked while
/// building: addresses and the subject are checked when sending (an invalid
/// recipient is a 400, a subject that is empty or spans several lines a 500).
/// Generated mailers (`ocre g mailer`) return one per action, rendered from
/// `templates/mailers/<name>/<action>.{txt,html}`.
///
/// It is serde-serializable because [`deliver_later`]
/// puts it in a queue message.
///
/// # Examples
///
/// ```
/// use ocre::mail::Email;
///
/// let email = Email::new("ada@example.com", "Reset your password", "Open https://example.com/reset/abc")
///     .html("<a href=\"https://example.com/reset/abc\">Reset your password</a>")
///     .reply_to("support@example.com");
/// assert_eq!(email.to, "ada@example.com");
/// assert_eq!(email.subject, "Reset your password");
/// assert_eq!(email.text, "Open https://example.com/reset/abc");
/// assert_eq!(email.html.as_deref(), Some("<a href=\"https://example.com/reset/abc\">Reset your password</a>"));
/// assert_eq!(email.reply_to.as_deref(), Some("support@example.com"));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Email {
    /// Recipient address, `ada@example.com`; an invalid one makes sending a 400.
    pub to: String,
    /// Subject line: one non-empty line of text.
    pub subject: String,
    /// Plain-text body; always sent, so every mail client can read it.
    pub text: String,
    /// Optional HTML body, shown instead of `text` by clients that render HTML.
    pub html: Option<String>,
    /// Where replies go, when not to `MAIL_FROM`; checked like `to`.
    pub reply_to: Option<String>,
}

impl Email {
    /// Creates a text-only email to one recipient.
    ///
    /// `html` and `reply_to` start empty. Nothing is validated here.
    ///
    /// # Examples
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Hi", "Hello Ada");
    /// assert_eq!((email.subject.as_str(), email.html, email.reply_to), ("Hi", None, None));
    /// ```
    pub fn new(to: impl Into<String>, subject: impl Into<String>, text: impl Into<String>) -> Self {
        Self { to: to.into(), subject: subject.into(), text: text.into(), html: None, reply_to: None }
    }

    /// Adds the HTML version of the body, replacing any previous one.
    ///
    /// The text body is still sent alongside it. The HTML is sent as given:
    /// escape user input when building it (askama templates do).
    ///
    /// # Examples
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Hi", "Hello").html("<p>Hello</p>");
    /// assert_eq!(email.html.as_deref(), Some("<p>Hello</p>"));
    /// ```
    pub fn html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }

    /// Sets the `Reply-To` address, so replies go there instead of to `MAIL_FROM`.
    ///
    /// Checked when sending: an invalid address is a 400, like the recipient.
    ///
    /// # Examples
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Hi", "Hello").reply_to("team@example.com");
    /// assert_eq!(email.reply_to.as_deref(), Some("team@example.com"));
    /// ```
    pub fn reply_to(mut self, address: impl Into<String>) -> Self {
        self.reply_to = Some(address.into());
        self
    }
}

/// How [`send`] delivers mail, from `MAIL_ADAPTER`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Adapter {
    Log,
    Resend,
    Cloudflare,
}

const ADAPTER_FIX: &str = "Fix: set MAIL_ADAPTER to \"resend\" (with the RESEND_API_KEY secret) or \"cloudflare\" \
                           (with a [[send_email]] binding named EMAIL) under [vars] in wrangler.toml; \
                           `ocre new` puts MAIL_ADAPTER=log in .dev.vars so `ocre dev` only logs mail";

/// Reads `MAIL_ADAPTER`. Unset is an error: production must choose, and
/// development gets `log` from `.dev.vars`.
pub(crate) fn adapter(value: Option<&str>) -> Result<Adapter> {
    match value.map(str::trim) {
        Some("log") => Ok(Adapter::Log),
        Some("resend") => Ok(Adapter::Resend),
        Some("cloudflare") => Ok(Adapter::Cloudflare),
        None => Err(Error::internal(format!("cannot send email: {MAIL_ADAPTER} is not set. {ADAPTER_FIX}"))),
        Some(other) => Err(Error::internal(format!(
            "cannot send email: unknown {MAIL_ADAPTER} \"{other}\" (expected log, resend or cloudflare). {ADAPTER_FIX}"
        ))),
    }
}

/// The Resend key, from the `RESEND_API_KEY` secret.
pub(crate) fn resend_key(secret: Option<String>) -> Result<String> {
    secret.filter(|key| !key.trim().is_empty()).ok_or_else(|| {
        Error::internal(format!(
            "cannot send email: the {RESEND_API_KEY} secret is not set. Fix: create a key at \
             https://resend.com/api-keys and run `npx wrangler secret put {RESEND_API_KEY}` \
             (and put it in .dev.vars to send from `ocre dev`)"
        ))
    })
}

/// An address with an optional display name: `Ada <ada@example.com>`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Mailbox {
    pub name: Option<String>,
    pub address: String,
}

impl Mailbox {
    /// `ada@example.com` or `Ada Lovelace <ada@example.com>` (the name may be quoted).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (name, address) = match text.strip_suffix('>').and_then(|rest| rest.rsplit_once('<')) {
            Some((name, address)) => {
                let name = name.trim();
                let name = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')).unwrap_or(name).trim();
                (Some(name.to_owned()).filter(|n| !n.is_empty()), address.trim())
            }
            None => (None, text),
        };
        let clean = name.as_deref().is_none_or(|n| !n.contains(|c: char| c.is_control() || c == '"'));
        (clean && is_email(address)).then(|| Self { name, address: address.to_owned() })
    }
}

impl std::fmt::Display for Mailbox {
    /// `ada@example.com`, `Ada <ada@example.com>`, or `"Acme, Inc." <x@acme.test>`
    /// when the name has characters that would split the header.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            None => f.write_str(&self.address),
            Some(name) if name.contains(|c| ",;:<>@()[]\\.".contains(c)) => write!(f, "\"{name}\" <{}>", self.address),
            Some(name) => write!(f, "{name} <{}>", self.address),
        }
    }
}

/// A checked [`Email`] with its sender, ready for an adapter.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outgoing {
    pub from: Mailbox,
    pub email: Email,
}

impl Outgoing {
    /// Checks the sender (`MAIL_FROM`) and the email. Bad configuration or
    /// code is an internal error; a bad recipient is a 400, since it
    /// usually comes from a form.
    pub fn new(from: Option<String>, email: Email) -> Result<Self> {
        let from = from.ok_or_else(|| {
            Error::internal(format!(
                "cannot send email: {MAIL_FROM} is not set. Fix: add {MAIL_FROM} = \"App <noreply@yourdomain.com>\" \
                 under [vars] in wrangler.toml"
            ))
        })?;
        let from = Mailbox::parse(&from).ok_or_else(|| {
            Error::internal(format!(
                "cannot send email: {MAIL_FROM} \"{from}\" is not an address. Fix: use \"noreply@yourdomain.com\" \
                 or \"App <noreply@yourdomain.com>\""
            ))
        })?;
        for address in std::iter::once(&email.to).chain(&email.reply_to) {
            if !is_email(address) {
                return Err(Error::bad_request(format!("invalid email address: {address}")));
            }
        }
        if email.subject.trim().is_empty() || email.subject.contains(['\r', '\n']) {
            return Err(Error::internal(format!(
                "cannot send email: the subject must be one non-empty line, got {:?}",
                email.subject
            )));
        }
        Ok(Self { from, email })
    }

    /// What the `log` adapter prints: headers, text and HTML, framed by
    /// [`LOG_PREFIX`] lines.
    pub fn log_text(&self) -> String {
        let Email { to, subject, text, html, reply_to } = &self.email;
        let mut out = format!("{LOG_PREFIX} not sent ({MAIL_ADAPTER} = \"log\")\nFrom: {}\nTo: {to}\n", self.from);
        if let Some(reply_to) = reply_to {
            out.push_str(&format!("Reply-To: {reply_to}\n"));
        }
        out.push_str(&format!("Subject: {subject}\n\n{text}\n"));
        if let Some(html) = html {
            out.push_str(&format!("{LOG_PREFIX} HTML version:\n{html}\n"));
        }
        out.push_str(&format!("{LOG_PREFIX} end"));
        out
    }

    /// Body of `POST https://api.resend.com/emails`.
    pub fn resend_json(&self) -> Value {
        let Email { to, subject, text, html, reply_to } = &self.email;
        let mut body = json!({ "from": self.from.to_string(), "to": [to], "subject": subject, "text": text });
        if let Some(html) = html {
            body["html"] = json!(html);
        }
        if let Some(reply_to) = reply_to {
            body["reply_to"] = json!(reply_to);
        }
        body
    }
}

/// Resend's endpoint for sending one email.
pub(crate) const RESEND_URL: &str = "https://api.resend.com/emails";

/// Error for a non-2xx Resend answer (`{"statusCode", "name", "message"}`).
pub(crate) fn resend_error(status: u16, body: &str) -> Error {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = parsed["message"].as_str().unwrap_or(body);
    let fix = match status {
        401 | 403 => {
            "check the RESEND_API_KEY secret, and that MAIL_FROM uses a domain verified at https://resend.com/domains"
        }
        429 => "Resend's rate or daily quota was reached (free plan: 100 emails a day, 3,000 a month)",
        _ => "see https://resend.com/docs/api-reference/errors",
    };
    Error::internal(format!("Resend did not send the email ({status}: {message}). Fix: {fix}"))
}

/// Error for a failed `send_email` binding call.
pub(crate) fn cloudflare_error(detail: &str) -> Error {
    Error::internal(format!(
        "Cloudflare Email Service did not send the email ({detail}). Fix: MAIL_FROM must use a domain onboarded to \
         Email Service; sending to any recipient needs the Workers Paid plan, while the free plan can only send \
         to verified destination addresses of the account (or use MAIL_ADAPTER = \"resend\")"
    ))
}

#[cfg(test)]
#[path = "../tests/mail.rs"]
mod tests;
