//! Email: sending with [`send`](crate::mail::send) and receiving with [`receive`](crate::mail::receive).
//!
//! ```ignore
//! use ocre::mail::{self, Email};
//!
//! let email = Email::new("ada@example.com", "Welcome", "Hello Ada,\n\nhttps://example.com/start\n")
//!     .html("<p>Hello Ada,</p><p><a href=\"https://example.com/start\">Start</a></p>");
//! mail::send(&ctx, email).await?;
//! ```
//!
//! The `MAIL_ADAPTER` Worker variable picks how mail leaves the Worker, with
//! no guessing: `log` (print it to the Worker console; `ocre new` sets it in
//! `.dev.vars` for `ocre dev`), `resend` (Resend's HTTP API, key in the
//! `RESEND_API_KEY` secret) or `cloudflare` (the `EMAIL` send_email binding).
//! The sender is the `MAIL_FROM` variable, `noreply@example.com` or
//! `Name <noreply@example.com>`.
//!
//! [`deliver_later`] sends from the background jobs queue instead (see
//! [`jobs`](crate::jobs)): the request does not wait for the provider, and a
//! failed delivery is retried.

mod parse;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use crate::runtime::mail::{InboundEmail, deliver_later, receive, send};
pub(crate) use parse::Message;

use crate::{Error, Result, validate::is_email};

/// Worker variable choosing the adapter: `log`, `resend` or `cloudflare`.
pub const MAIL_ADAPTER: &str = "MAIL_ADAPTER";
/// Worker variable with the sender: `noreply@example.com` or `Name <noreply@example.com>`.
pub const MAIL_FROM: &str = "MAIL_FROM";
/// Worker secret with the Resend API key (`MAIL_ADAPTER = "resend"`).
pub const RESEND_API_KEY: &str = "RESEND_API_KEY";
/// Name of the `[[send_email]]` binding (`MAIL_ADAPTER = "cloudflare"`).
pub const EMAIL_BINDING: &str = "EMAIL";
/// Prefix of every line Ocre logs about mail, e.g. in `ocre dev` output.
pub const LOG_PREFIX: &str = "[ocre mail]";

/// An email to send with [`send`]. Build it with [`Email::new`], then add an
/// HTML version with [`html`](Self::html):
///
/// ```
/// use ocre::mail::Email;
///
/// let email = Email::new("ada@example.com", "Reset your password", "Open https://example.com/reset/abc")
///     .html("<a href=\"https://example.com/reset/abc\">Reset your password</a>")
///     .reply_to("support@example.com");
/// assert_eq!(email.to, "ada@example.com");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Email {
    /// Recipient address, `ada@example.com`.
    pub to: String,
    /// One line of text.
    pub subject: String,
    /// Plain-text body; always sent, so every mail client can read it.
    pub text: String,
    /// Optional HTML body, shown instead of `text` by clients that render HTML.
    pub html: Option<String>,
    /// Where replies go, when not to `MAIL_FROM`.
    pub reply_to: Option<String>,
}

impl Email {
    /// Text-only email to one recipient.
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Hi", "Hello Ada");
    /// assert_eq!((email.subject.as_str(), email.html), ("Hi", None));
    /// ```
    pub fn new(to: impl Into<String>, subject: impl Into<String>, text: impl Into<String>) -> Self {
        Self { to: to.into(), subject: subject.into(), text: text.into(), html: None, reply_to: None }
    }

    /// Adds the HTML version of the body.
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Hi", "Hello").html("<p>Hello</p>");
    /// assert_eq!(email.html.as_deref(), Some("<p>Hello</p>"));
    /// ```
    pub fn html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }

    /// Sets the `Reply-To` address.
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
