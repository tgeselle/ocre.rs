//! Email: send with adapters (log, Resend, Cloudflare), receive from Email Routing.
//!
//! Sending is Rails' Action Mailer without the class: build an [`Email`]
//! (several recipients, cc, bcc, headers, attachments and inline images) and
//! hand it to [`send`], or to [`deliver_later`] / [`deliver_in`] to send it
//! from the background jobs queue (see [`jobs`](crate::jobs)) so the request
//! does not wait for the provider and a failed delivery is retried.
//! Receiving is Action Mailbox: [`receive`] is the Worker's `email` entry
//! point and hands each message to the app as an [`InboundEmail`]. In
//! `ocre dev`, [`dev_routes`] serves mailer previews, the emails sent, and a
//! form that delivers test email to the mailbox.
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
//! - `log`: prints the whole email (headers, text, HTML, attachments) to the
//!   Worker console between [`LOG_PREFIX`] lines, like Rails'
//!   letter_opener, and in debug builds keeps the last 20 for
//!   `/ocre/dev/mailers` (see [`dev_routes`]). `ocre new` writes
//!   `MAIL_ADAPTER=log` to `.dev.vars`, which overrides `[vars]` in
//!   `ocre dev`. No configuration, no limits.
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
//! The sender is the email's own [`from`](Email::from), else the
//! [`MAIL_FROM`] variable, `noreply@example.com` or
//! `Name <noreply@example.com>`. With `MAIL_ADAPTER`
//! unset, sending fails with an [`Error::Internal`]
//! naming the fix, so a production Worker never drops mail silently. For
//! sign-up and password-reset mail on the free plan, use Resend.
//!
//! Receiving uses Cloudflare Email Routing, free and unlimited on every plan.
//! Every line Ocre logs about mail starts with [`LOG_PREFIX`].

mod dev;
mod parse;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use crate::runtime::mail::{InboundEmail, deliver_in, deliver_later, receive, send};
pub(crate) use dev::capture;
pub use dev::{Preview, dev_routes};
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

/// An outgoing email: recipients, a subject, a plain-text body, and optionally HTML, headers and attachments.
///
/// Build it with [`Email::new`] (one recipient), then add more with
/// [`also_to`](Self::also_to), [`cc`](Self::cc) and [`bcc`](Self::bcc), an
/// HTML version with [`html`](Self::html), a `Reply-To` with
/// [`reply_to`](Self::reply_to), another sender with [`from`](Self::from),
/// custom headers with [`header`](Self::header), and files with
/// [`attach`](Self::attach) and [`inline`](Self::inline); send it with
/// [`send`] or [`deliver_later`]. Nothing is checked while building:
/// addresses, headers, attachments and the subject are checked when sending
/// (an invalid recipient is a 400, the rest a 500 naming the fix). Every
/// address may carry a display name, `Ada <ada@example.com>` (see
/// [`address_with_name`]). Generated mailers (`ocre g mailer`) return one per
/// action, rendered from `templates/mailers/<name>/<action>.{txt,html}`.
///
/// It is serde-serializable because [`deliver_later`] puts it in a queue
/// message (128 KB at most, attachments included, base64-encoded).
///
/// # Examples
///
/// ```
/// use ocre::mail::Email;
///
/// let email = Email::new("ada@example.com", "Reset your password", "Open https://example.com/reset/abc")
///     .html("<a href=\"https://example.com/reset/abc\">Reset your password</a>")
///     .reply_to("support@example.com");
/// assert_eq!(email.to, ["ada@example.com"]);
/// assert_eq!(email.subject, "Reset your password");
/// assert_eq!(email.text, "Open https://example.com/reset/abc");
/// assert_eq!(email.html.as_deref(), Some("<a href=\"https://example.com/reset/abc\">Reset your password</a>"));
/// assert_eq!(email.reply_to.as_deref(), Some("support@example.com"));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Email {
    /// Sender, when not `MAIL_FROM`: `billing@example.com` or `Billing <billing@example.com>`,
    /// on a domain verified with the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `To` recipients, `ada@example.com` or `Ada <ada@example.com>`; an invalid one makes sending a 400.
    #[serde(deserialize_with = "one_or_many")]
    pub to: Vec<String>,
    /// `Cc` recipients, checked like `to`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    /// `Bcc` recipients, checked like `to`; the other recipients do not see them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,
    /// Subject line: one non-empty line of text.
    pub subject: String,
    /// Plain-text body; always sent, so every mail client can read it.
    pub text: String,
    /// Optional HTML body, shown instead of `text` by clients that render HTML.
    pub html: Option<String>,
    /// Where replies go, when not to the sender; checked like `to`.
    pub reply_to: Option<String>,
    /// Extra headers, `(name, value)`: threading (`In-Reply-To`, `References`), `List-Unsubscribe`, `X-...`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    /// Files attached to the email, and inline images the HTML shows with `cid:`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

/// A file attached to an [`Email`], or an inline image shown by its HTML.
///
/// Built by [`Email::attach`] and [`Email::inline`]; also what
/// [`InboundEmail::attachments`] returns for received mail. In a queue
/// message the content travels base64-encoded.
///
/// # Examples
///
/// ```
/// let email = ocre::mail::Email::new("ada@example.com", "Invoice", "Attached.")
///     .attach("invoice.pdf", "application/pdf", b"%PDF-1.7".to_vec());
/// let file = &email.attachments[0];
/// assert_eq!((file.filename.as_str(), file.content_type.as_str(), file.content_id.as_deref()),
///            ("invoice.pdf", "application/pdf", None));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    /// File name shown by mail clients, `invoice.pdf`.
    pub filename: String,
    /// MIME type, `application/pdf` or `image/png`.
    pub content_type: String,
    /// The file's bytes.
    #[serde(with = "base64_bytes")]
    pub content: Vec<u8>,
    /// For inline images: the id the HTML refers to as `cid:<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
}

impl Email {
    /// Creates a text-only email to one recipient.
    ///
    /// Everything else starts empty. Nothing is validated here.
    ///
    /// # Examples
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Hi", "Hello Ada");
    /// assert_eq!((email.subject.as_str(), email.html, email.reply_to), ("Hi", None, None));
    /// ```
    pub fn new(to: impl Into<String>, subject: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            from: None,
            to: vec![to.into()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: subject.into(),
            text: text.into(),
            html: None,
            reply_to: None,
            headers: Vec::new(),
            attachments: Vec::new(),
        }
    }

    /// Adds another `To` recipient.
    ///
    /// All `To` and `Cc` recipients see each other; use [`bcc`](Self::bcc)
    /// or one email each to keep addresses private. 50 recipients at most
    /// (`to`, `cc` and `bcc` together), Resend's limit.
    ///
    /// # Examples
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Hi", "Hello").also_to("Grace <grace@example.com>");
    /// assert_eq!(email.to, ["ada@example.com", "Grace <grace@example.com>"]);
    /// ```
    pub fn also_to(mut self, address: impl Into<String>) -> Self {
        self.to.push(address.into());
        self
    }

    /// Adds a `Cc` recipient.
    ///
    /// # Examples
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Hi", "Hello").cc("team@example.com");
    /// assert_eq!(email.cc, ["team@example.com"]);
    /// ```
    pub fn cc(mut self, address: impl Into<String>) -> Self {
        self.cc.push(address.into());
        self
    }

    /// Adds a `Bcc` recipient: it gets the email, the other recipients do not see it.
    ///
    /// # Examples
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Hi", "Hello").bcc("archive@example.com");
    /// assert_eq!(email.bcc, ["archive@example.com"]);
    /// ```
    pub fn bcc(mut self, address: impl Into<String>) -> Self {
        self.bcc.push(address.into());
        self
    }

    /// Sends from `address` instead of the `MAIL_FROM` variable.
    ///
    /// Like Rails' `mail(from: ...)`; the domain must be verified with the
    /// provider, like `MAIL_FROM`'s. An unparsable sender is a 500.
    ///
    /// # Examples
    ///
    /// ```
    /// let email = ocre::mail::Email::new("ada@example.com", "Invoice", "...").from("Billing <billing@example.com>");
    /// assert_eq!(email.from.as_deref(), Some("Billing <billing@example.com>"));
    /// ```
    pub fn from(mut self, address: impl Into<String>) -> Self {
        self.from = Some(address.into());
        self
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

    /// Sets the `Reply-To` address, so replies go there instead of to the sender.
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

    /// Adds a header, e.g. `In-Reply-To` and `References` to thread a reply, or `List-Unsubscribe`.
    ///
    /// When sending, the name must be letters, digits and `-`, the value one
    /// line, and the name not one Ocre sets itself (`From`, `To`, `Cc`,
    /// `Bcc`, `Subject`, `Reply-To`, `Content-Type`, ...); otherwise sending
    /// is a 500 naming the header.
    ///
    /// # Examples
    ///
    /// ```
    /// let reply = ocre::mail::Email::new("ada@example.com", "Re: Order 42", "Shipped today.")
    ///     .header("In-Reply-To", "<order-42@example.com>")
    ///     .header("References", "<order-42@example.com>");
    /// assert_eq!(reply.headers[0], ("In-Reply-To".to_owned(), "<order-42@example.com>".to_owned()));
    /// ```
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Attaches a file, like Rails' `attachments["invoice.pdf"] = bytes`.
    ///
    /// The whole email must fit the provider's limit (Resend: 40 MB), and
    /// 128 KB for [`deliver_later`] (base64 makes files a third larger):
    /// store big files in R2 and send a link instead.
    ///
    /// # Examples
    ///
    /// ```
    /// let csv = "id,total\n1,42\n".as_bytes().to_vec();
    /// let email = ocre::mail::Email::new("ada@example.com", "Report", "Attached.").attach("report.csv", "text/csv", csv);
    /// assert_eq!(email.attachments[0].content, b"id,total\n1,42\n");
    /// ```
    pub fn attach(mut self, filename: impl Into<String>, content_type: impl Into<String>, content: Vec<u8>) -> Self {
        self.attachments.push(Attachment {
            filename: filename.into(),
            content_type: content_type.into(),
            content,
            content_id: None,
        });
        self
    }

    /// Embeds an image that the HTML shows with `<img src="cid:<content_id>">`, like Rails' `attachments.inline`.
    ///
    /// `content_id` is letters, digits and `.-_` (checked when sending).
    /// Mail clients show inline images without loading anything remote;
    /// the text body cannot show them.
    ///
    /// # Examples
    ///
    /// ```
    /// let logo = vec![0x89, b'P', b'N', b'G'];
    /// let email = ocre::mail::Email::new("ada@example.com", "Welcome", "Welcome!")
    ///     .html("<img src=\"cid:logo\" alt=\"Shop\"><p>Welcome!</p>")
    ///     .inline("logo", "logo.png", "image/png", logo);
    /// assert_eq!(email.attachments[0].content_id.as_deref(), Some("logo"));
    /// ```
    pub fn inline(
        mut self,
        content_id: impl Into<String>,
        filename: impl Into<String>,
        content_type: impl Into<String>,
        content: Vec<u8>,
    ) -> Self {
        self.attachments.push(Attachment {
            filename: filename.into(),
            content_type: content_type.into(),
            content,
            content_id: Some(content_id.into()),
        });
        self
    }
}

/// Formats `Name <address>`, quoting the name when needed, like Rails' `email_address_with_name`.
///
/// Double quotes and line breaks in `name` become spaces, so a user-typed
/// name cannot break the header. An empty name gives the bare address.
///
/// # Examples
///
/// ```
/// use ocre::mail::address_with_name;
///
/// assert_eq!(address_with_name("Ada Lovelace", "ada@example.com"), "Ada Lovelace <ada@example.com>");
/// assert_eq!(address_with_name("Acme, Inc.", "x@acme.test"), "\"Acme, Inc.\" <x@acme.test>");
/// assert_eq!(address_with_name(" ", "ada@example.com"), "ada@example.com");
/// ```
pub fn address_with_name(name: &str, address: &str) -> String {
    let name: String = name.chars().map(|c| if c == '"' || c.is_control() { ' ' } else { c }).collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    Mailbox { name: Some(name).filter(|n| !n.is_empty()), address: address.trim().to_owned() }.to_string()
}

/// `to` as a list, or as one address (messages queued before `to` became a list).
fn one_or_many<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(address) => vec![address],
        OneOrMany::Many(addresses) => addresses,
    })
}

/// Attachment bytes as base64 text in JSON.
mod base64_bytes {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        STANDARD.decode(String::deserialize(deserializer)?).map_err(D::Error::custom)
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

/// Most recipients per email (`to`, `cc` and `bcc` together): Resend's limit.
pub(crate) const MAX_RECIPIENTS: usize = 50;

/// Headers Ocre writes itself; [`Email::header`] refuses them.
const RESERVED_HEADERS: [&str; 10] = [
    "from",
    "to",
    "cc",
    "bcc",
    "subject",
    "reply-to",
    "content-type",
    "content-transfer-encoding",
    "mime-version",
    "date",
];

/// A checked [`Email`] with its sender, ready for an adapter.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outgoing {
    pub from: Mailbox,
    pub email: Email,
}

impl Outgoing {
    /// Checks the sender (the email's own, else `MAIL_FROM`) and the email.
    /// Bad configuration or code is an internal error; a bad recipient is a
    /// 400, since it usually comes from a form.
    pub fn new(mail_from: Option<String>, email: Email) -> Result<Self> {
        let (from, source) = match &email.from {
            Some(from) => (from.clone(), "the email's `from`"),
            None => (
                mail_from.ok_or_else(|| {
                    Error::internal(format!(
                        "cannot send email: {MAIL_FROM} is not set. Fix: add {MAIL_FROM} = \"App \
                         <noreply@yourdomain.com>\" under [vars] in wrangler.toml"
                    ))
                })?,
                MAIL_FROM,
            ),
        };
        let from = Mailbox::parse(&from).ok_or_else(|| {
            Error::internal(format!(
                "cannot send email: {source} \"{from}\" is not an address. Fix: use \"noreply@yourdomain.com\" \
                 or \"App <noreply@yourdomain.com>\""
            ))
        })?;
        if email.to.is_empty() {
            return Err(Error::internal("cannot send email: it has no `to` recipient"));
        }
        let recipients = email.to.iter().chain(&email.cc).chain(&email.bcc);
        for address in recipients.clone().chain(&email.reply_to) {
            if Mailbox::parse(address).is_none() {
                return Err(Error::bad_request(format!("invalid email address: {address}")));
            }
        }
        let count = recipients.count();
        if count > MAX_RECIPIENTS {
            return Err(Error::internal(format!(
                "cannot send email to {count} recipients: {MAX_RECIPIENTS} at most (to, cc and bcc). Fix: send \
                 one email per recipient, e.g. one `deliver_later` each"
            )));
        }
        if email.subject.trim().is_empty() || email.subject.contains(['\r', '\n']) {
            return Err(Error::internal(format!(
                "cannot send email: the subject must be one non-empty line, got {:?}",
                email.subject
            )));
        }
        for (name, value) in &email.headers {
            let token = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !token || value.contains(['\r', '\n']) || RESERVED_HEADERS.contains(&name.to_ascii_lowercase().as_str())
            {
                return Err(Error::internal(format!(
                    "cannot send email: header {name:?}: {value:?} is not allowed. Fix: use a name of letters, \
                     digits and `-` that Ocre does not set itself (use `.cc`, `.reply_to`, ... for those), and a \
                     one-line value"
                )));
            }
        }
        for file in &email.attachments {
            let filename = !file.filename.trim().is_empty() && !file.filename.contains(|c: char| c.is_control());
            let content_type = file.content_type.split_once('/').is_some_and(|(kind, sub)| {
                !kind.is_empty() && !sub.is_empty() && !file.content_type.contains(|c: char| c.is_whitespace())
            });
            let content_id = file
                .content_id
                .as_deref()
                .is_none_or(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)));
            if !(filename && content_type && content_id) {
                return Err(Error::internal(format!(
                    "cannot send email: attachment {:?} ({:?}, content id {:?}) is invalid. Fix: give a file \
                     name, a MIME type such as \"application/pdf\", and a content id of letters, digits and .-_",
                    file.filename, file.content_type, file.content_id
                )));
            }
        }
        Ok(Self { from, email })
    }

    /// Addresses of `list`, without display names (the Cloudflare binding takes bare addresses).
    pub fn addresses(list: &[String]) -> Vec<String> {
        list.iter().filter_map(|address| Mailbox::parse(address)).map(|mailbox| mailbox.address).collect()
    }

    /// What the `log` adapter prints: headers, text, HTML and attachments,
    /// framed by [`LOG_PREFIX`] lines.
    pub fn log_text(&self) -> String {
        let email = &self.email;
        let mut out = format!(
            "{LOG_PREFIX} not sent ({MAIL_ADAPTER} = \"log\")\nFrom: {}\nTo: {}\n",
            self.from,
            email.to.join(", ")
        );
        for (name, list) in [("Cc", &email.cc), ("Bcc", &email.bcc)] {
            if !list.is_empty() {
                out.push_str(&format!("{name}: {}\n", list.join(", ")));
            }
        }
        if let Some(reply_to) = &email.reply_to {
            out.push_str(&format!("Reply-To: {reply_to}\n"));
        }
        for (name, value) in &email.headers {
            out.push_str(&format!("{name}: {value}\n"));
        }
        out.push_str(&format!("Subject: {}\n\n{}\n", email.subject, email.text));
        if let Some(html) = &email.html {
            out.push_str(&format!("{LOG_PREFIX} HTML version:\n{html}\n"));
        }
        for file in &email.attachments {
            let kind = match &file.content_id {
                Some(id) => format!("inline cid:{id}"),
                None => "attachment".to_owned(),
            };
            out.push_str(&format!(
                "{LOG_PREFIX} {kind}: {} ({}, {} bytes)\n",
                file.filename,
                file.content_type,
                file.content.len()
            ));
        }
        out.push_str(&format!("{LOG_PREFIX} end"));
        out
    }

    /// Body of `POST https://api.resend.com/emails`.
    pub fn resend_json(&self) -> Value {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let email = &self.email;
        let mut body =
            json!({ "from": self.from.to_string(), "to": email.to, "subject": email.subject, "text": email.text });
        for (key, list) in [("cc", &email.cc), ("bcc", &email.bcc)] {
            if !list.is_empty() {
                body[key] = json!(list);
            }
        }
        if let Some(html) = &email.html {
            body["html"] = json!(html);
        }
        if let Some(reply_to) = &email.reply_to {
            body["reply_to"] = json!(reply_to);
        }
        if !email.headers.is_empty() {
            body["headers"] = email.headers.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
        }
        if !email.attachments.is_empty() {
            let files = email.attachments.iter().map(|file| {
                let mut entry = json!({
                    "filename": file.filename,
                    "content": STANDARD.encode(&file.content),
                    "content_type": file.content_type,
                });
                if let Some(id) = &file.content_id {
                    entry["content_id"] = json!(id);
                }
                entry
            });
            body["attachments"] = files.collect();
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
