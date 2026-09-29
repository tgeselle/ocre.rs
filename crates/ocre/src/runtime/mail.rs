use wasm_bindgen::JsValue;
use worker::{
    EmailAddress, Env, Fetch, ForwardableEmailMessage, Headers, Method, Request, RequestInit, SendEmailBuilder,
    send::SendFuture,
};

use super::Ctx;
use crate::{
    Error, Result,
    mail::{
        Adapter, EMAIL_BINDING, Email, LOG_PREFIX, MAIL_ADAPTER, MAIL_FROM, Message, Outgoing, RESEND_API_KEY,
        RESEND_URL, adapter, cloudflare_error, resend_error, resend_key,
    },
};

/// Sends `email` from `MAIL_FROM` with the adapter named by `MAIL_ADAPTER`:
/// `log` prints it to the Worker console (lines starting with `[ocre mail]`),
/// `resend` calls Resend's API, `cloudflare` uses the `EMAIL` send_email
/// binding. Errors name the missing configuration.
///
/// ```ignore
/// async fn invite(State(ctx): State<Ctx>, Form(form): Form<InviteForm>) -> ocre::Result<Redirect> {
///     let email = Email::new(&form.email, "You're invited", format!("Join us: {}", form.link));
///     ocre::mail::send(&ctx, email).await?;
///     Ok(Redirect::to("/"))
/// }
/// ```
///
/// The returned future is `Send`, so axum handlers can await it.
pub fn send(ctx: &Ctx, email: Email) -> impl Future<Output = Result<()>> + Send + use<> {
    let env = ctx.env().clone();
    SendFuture::new(async move { deliver(&env, email).await })
}

fn var(env: &Env, name: &str) -> Option<String> {
    env.var(name).ok().map(|value| value.to_string())
}

async fn deliver(env: &Env, email: Email) -> Result<()> {
    let adapter = adapter(var(env, MAIL_ADAPTER).as_deref())?;
    let outgoing = Outgoing::new(var(env, MAIL_FROM), email)?;
    match adapter {
        Adapter::Log => {
            worker::console_log!("{}", outgoing.log_text());
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
    let builder = match &from.name {
        Some(name) => SendEmailBuilder::builder_with_email_address_and_str(
            &EmailAddress::new(name, &from.address),
            &email.to,
            &email.subject,
        ),
        None => SendEmailBuilder::builder(&from.address, &email.to, &email.subject),
    };
    let mut builder = builder.text(&email.text);
    if let Some(html) = &email.html {
        builder = builder.html(html);
    }
    if let Some(reply_to) = &email.reply_to {
        builder = builder.reply_to(reply_to);
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

/// An email that Cloudflare Email Routing delivered to the Worker. Read it
/// with [`subject`](Self::subject), [`text`](Self::text), [`header`](Self::header)...;
/// bounce it with [`reject`](Self::reject) or pass it on with [`forward`](Self::forward).
///
/// ```ignore
/// // src/mailbox.rs
/// pub async fn receive(ctx: Ctx, email: InboundEmail) -> ocre::Result<()> {
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
    /// Envelope sender (SMTP `MAIL FROM`), checked by Cloudflare. The
    /// `From` header may differ: `email.header("From")`.
    pub fn from(&self) -> &str {
        &self.from
    }

    /// Envelope recipient: the address of this app that received the email.
    pub fn to(&self) -> &str {
        &self.to
    }

    /// `Subject` header, decoded; empty when missing.
    pub fn subject(&self) -> &str {
        self.message.header("Subject").unwrap_or("")
    }

    /// First header with this name (case-insensitive), decoded:
    /// `email.header("Message-ID")`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.message.header(name)
    }

    /// Every header, in order, as `(name, value)`.
    pub fn headers(&self) -> &[(String, String)] {
        &self.message.headers
    }

    /// The first `text/plain` part, decoded to UTF-8.
    pub fn text(&self) -> Option<&str> {
        self.message.text.as_deref()
    }

    /// The first `text/html` part, decoded to UTF-8. Never render it unescaped.
    pub fn html(&self) -> Option<&str> {
        self.message.html.as_deref()
    }

    /// The whole message as received (RFC 5322), e.g. to store it or read attachments.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// Bounces the email: the sending server gets a permanent SMTP error with `reason`.
    pub fn reject(&self, reason: &str) {
        self.inner.set_reject(reason);
    }

    /// Forwards the email unchanged to `to`, which must be a verified
    /// destination address of the Cloudflare account.
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

/// Runs `handler` for an email from Cloudflare Email Routing. Call it from
/// the Worker's email entry point (`ocre g mailbox` writes this):
///
/// ```ignore
/// #[worker::event(email)]
/// async fn email(message: worker::ForwardableEmailMessage, env: worker::Env, _ctx: worker::Context)
///     -> worker::Result<()> {
///     ocre::mail::receive(message, env, mailbox::receive).await
/// }
/// ```
///
/// Logs one `[ocre mail] received ...` line per email. When the handler
/// fails, the error is logged and the email bounced ("could not be
/// processed"), so the sender knows it was not handled.
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
