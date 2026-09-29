//! Development pages under `/ocre/dev/`: mailer previews, the emails the
//! `log` adapter captured, and a form that delivers a test email to the
//! app's mailbox. Compiled into debug builds only (`ocre dev`); release
//! builds (`ocre deploy`) get an empty router.

use axum::Router;
#[cfg(debug_assertions)]
use axum::{
    extract::Path,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};

use super::Email;
#[cfg(debug_assertions)]
use super::{Attachment, Outgoing};
use crate::Result;

/// A mailer action shown at `/ocre/dev/mailers` in `ocre dev`, rendered with sample data, like Rails' mailer previews.
///
/// `name` reads `mailer/action`; `build` returns the email, usually by
/// calling the action with sample arguments. `ocre g mailer` adds one per
/// action to `PREVIEWS` in `src/mailers/mod.rs`. Nothing is sent: the page
/// shows the headers, the HTML (inline images included) and the text.
///
/// # Examples
///
/// ```
/// use ocre::mail::{Email, Preview};
///
/// fn welcome(to: &str) -> ocre::Result<Email> {
///     Ok(Email::new(to, "Welcome", "Hello"))
/// }
///
/// static PREVIEWS: &[Preview] = &[Preview::new("user/welcome", || welcome("ada@example.com"))];
/// assert_eq!(PREVIEWS[0].name, "user/welcome");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Preview {
    /// `mailer/action`, e.g. `user/welcome`; the page's URL is `/ocre/dev/mailers/preview/<name>`.
    pub name: &'static str,
    /// Builds the email with sample data.
    pub build: fn() -> Result<Email>,
}

impl Preview {
    /// A preview named `mailer/action`, built by `build`.
    ///
    /// # Examples
    ///
    /// ```
    /// let preview = ocre::mail::Preview::new("user/welcome", || Ok(ocre::mail::Email::new("ada@example.com", "Hi", "Hello")));
    /// assert_eq!((preview.build)().unwrap().subject, "Hi");
    /// ```
    pub const fn new(name: &'static str, build: fn() -> Result<Email>) -> Self {
        Self { name, build }
    }
}

/// Development pages for email, served by `ocre dev` only, like Rails' `/rails/mailers` and Action Mailbox's conductor.
///
/// - `GET /ocre/dev/mailers`: the `previews` and the last 20 emails sent
///   with `MAIL_ADAPTER = "log"` by this Worker instance (requests, jobs and
///   `deliver_later` alike), newest first.
/// - `GET /ocre/dev/mailers/preview/<name>`: one preview, rendered now.
/// - `GET /ocre/dev/mailers/sent/<id>`: one captured email.
/// - `GET /ocre/dev/mailers/sent.json`: the captured emails as JSON
///   (`[{"id": 1, "from": "...", "email": {...}}]`, oldest first), so an
///   end-to-end test can read a magic link without parsing logs.
/// - `GET /ocre/dev/mailbox`: a form that delivers a test email to the
///   app's mailbox (`ocre g mailbox`) through wrangler's local
///   `/cdn-cgi/local/email` endpoint.
///
/// The pages exist in debug builds only (`ocre dev` builds with `--dev`);
/// in release builds (`ocre deploy`) this returns an empty router, so they
/// are 404s in production. `ocre g mailer` and `ocre g mailbox` merge it
/// into `routes()` in `src/lib.rs`. It uses no billed resource: previews
/// render in the request, captured emails live in the Worker's memory.
///
/// # Examples
///
/// ```
/// use axum::Router;
/// use ocre::{Ctx, mail::Preview};
///
/// static PREVIEWS: &[Preview] = &[];
///
/// fn routes() -> Router<Ctx> {
///     Router::new()
///         // ocre:routes
///         .merge(ocre::mail::dev_routes(PREVIEWS))
/// }
/// # let _ = routes;
/// ```
pub fn dev_routes<S: Clone + Send + Sync + 'static>(previews: &'static [Preview]) -> Router<S> {
    #[cfg(not(debug_assertions))]
    {
        let _ = previews;
        Router::new()
    }
    #[cfg(debug_assertions)]
    Router::new()
        .route("/ocre/dev/mailers", get(move || async move { index(previews) }))
        .route(
            "/ocre/dev/mailers/preview/{*name}",
            get(move |Path(name): Path<String>| async move { preview(previews, &name) }),
        )
        .route("/ocre/dev/mailers/sent.json", get(|| async { sent_json() }))
        .route("/ocre/dev/mailers/sent/{id}", get(|Path(id): Path<String>| async move { sent(&id) }))
        .route("/ocre/dev/mailbox", get(|| async { page("Mailbox", MAILBOX_FORM.to_owned()) }))
        .route("/ocre/dev/mailbox.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], MAILBOX_JS) }))
}

/// Emails the `log` adapter printed, kept for the development pages.
#[cfg(debug_assertions)]
static SENT: std::sync::Mutex<Captured> = std::sync::Mutex::new(Captured { next: 1, emails: Vec::new() });

/// How many captured emails are kept.
#[cfg(debug_assertions)]
const KEEP: usize = 20;

#[cfg(debug_assertions)]
struct Captured {
    next: u64,
    emails: Vec<(u64, Outgoing)>,
}

/// Keeps an email the `log` adapter printed (debug builds only).
pub(crate) fn capture(outgoing: super::Outgoing) {
    #[cfg(debug_assertions)]
    {
        let mut sent = SENT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = sent.next;
        sent.next += 1;
        sent.emails.push((id, outgoing));
        if sent.emails.len() > KEEP {
            sent.emails.remove(0);
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = outgoing;
}

#[cfg(debug_assertions)]
fn captured() -> Vec<(u64, Outgoing)> {
    SENT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).emails.clone()
}

#[cfg(debug_assertions)]
fn index(previews: &[Preview]) -> Response {
    let mut body = String::from("<h1>Mailer previews</h1>");
    if previews.is_empty() {
        body.push_str(
            "<p>No previews: <code>ocre g mailer</code> adds them to <code>PREVIEWS</code> in src/mailers/mod.rs.</p>",
        );
    } else {
        body.push_str("<ul>");
        for preview in previews {
            let name = escape(preview.name);
            body.push_str(&format!("<li><a href=\"/ocre/dev/mailers/preview/{name}\">{name}</a></li>"));
        }
        body.push_str("</ul>");
    }
    body.push_str("<h2>Sent (MAIL_ADAPTER = \"log\")</h2>");
    let sent = captured();
    if sent.is_empty() {
        body.push_str("<p>Nothing sent since this Worker started.</p>");
    } else {
        body.push_str("<ul>");
        for (id, outgoing) in sent.iter().rev() {
            body.push_str(&format!(
                "<li><a href=\"/ocre/dev/mailers/sent/{id}\">{}</a> to {}</li>",
                escape(&outgoing.email.subject),
                escape(&outgoing.email.to.join(", "))
            ));
        }
        body.push_str("</ul>");
    }
    body.push_str("<p><a href=\"/ocre/dev/mailbox\">Deliver a test email to the mailbox</a></p>");
    page("Mailers", body)
}

#[cfg(debug_assertions)]
fn preview(previews: &[Preview], name: &str) -> Response {
    let Some(preview) = previews.iter().find(|preview| preview.name == name) else {
        return (StatusCode::NOT_FOUND, page("Not found", format!("<p>No preview named {}.</p>", escape(name))))
            .into_response();
    };
    match (preview.build)() {
        Ok(email) => page(name, email_html(None, &email)),
        Err(err) => {
            let body = format!("<h1>{}</h1><p>The preview failed: {}</p>", escape(name), escape(&err.to_string()));
            (StatusCode::INTERNAL_SERVER_ERROR, page(name, body)).into_response()
        }
    }
}

#[cfg(debug_assertions)]
fn sent(id: &str) -> Response {
    match captured().into_iter().find(|(sent, _)| sent.to_string() == id) {
        Some((_, outgoing)) => {
            page(&outgoing.email.subject, email_html(Some(&outgoing.from.to_string()), &outgoing.email))
        }
        None => {
            (StatusCode::NOT_FOUND, page("Not found", "<p>No such email; only the last 20 are kept.</p>".to_owned()))
                .into_response()
        }
    }
}

#[cfg(debug_assertions)]
fn sent_json() -> Response {
    let list: Vec<serde_json::Value> = captured()
        .into_iter()
        .map(|(id, outgoing)| serde_json::json!({ "id": id, "from": outgoing.from.to_string(), "email": outgoing.email }))
        .collect();
    axum::Json(list).into_response()
}

/// Headers, the HTML in a sandboxed frame (inline images as data URLs), the text and the attachments.
#[cfg(debug_assertions)]
fn email_html(from: Option<&str>, email: &Email) -> String {
    let mut rows = Vec::new();
    rows.push((
        "From",
        from.map(str::to_owned).or_else(|| email.from.clone()).unwrap_or_else(|| "MAIL_FROM".to_owned()),
    ));
    rows.push(("To", email.to.join(", ")));
    for (name, list) in [("Cc", &email.cc), ("Bcc", &email.bcc)] {
        if !list.is_empty() {
            rows.push((name, list.join(", ")));
        }
    }
    if let Some(reply_to) = &email.reply_to {
        rows.push(("Reply-To", reply_to.clone()));
    }
    rows.push(("Subject", email.subject.clone()));
    let mut out = format!("<h1>{}</h1><table>", escape(&email.subject));
    for (name, value) in
        rows.iter().map(|(n, v)| (*n, v.as_str())).chain(email.headers.iter().map(|(n, v)| (n.as_str(), v.as_str())))
    {
        out.push_str(&format!("<tr><th>{}</th><td>{}</td></tr>", escape(name), escape(value)));
    }
    out.push_str("</table>");
    for file in &email.attachments {
        let kind = file.content_id.as_ref().map_or_else(|| "Attachment".to_owned(), |id| format!("Inline cid:{id}"));
        out.push_str(&format!(
            "<p>{}: {} ({}, {} bytes)</p>",
            escape(&kind),
            escape(&file.filename),
            escape(&file.content_type),
            file.content.len()
        ));
    }
    if let Some(html) = &email.html {
        let html = inline_images(html, &email.attachments);
        out.push_str(&format!("<h2>HTML</h2><iframe sandbox srcdoc=\"{}\"></iframe>", escape(&html)));
    }
    out.push_str(&format!("<h2>Text</h2><pre>{}</pre>", escape(&email.text)));
    out
}

/// `cid:<id>` references replaced by `data:` URLs, so the browser shows inline images.
#[cfg(debug_assertions)]
fn inline_images(html: &str, attachments: &[Attachment]) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut html = html.to_owned();
    for file in attachments {
        if let Some(id) = &file.content_id {
            let data = format!("data:{};base64,{}", file.content_type, STANDARD.encode(&file.content));
            html = html.replace(&format!("cid:{id}"), &data);
        }
    }
    html
}

/// A development page, with its own Content-Security-Policy: no inline
/// scripts, images from anywhere, the email HTML in a sandboxed frame.
#[cfg(debug_assertions)]
fn page(title: &str, body: String) -> Response {
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{} · ocre dev</title><style>{STYLE}</style></head>\
         <body><nav><a href=\"/ocre/dev/mailers\">Mailers</a> · <a href=\"/ocre/dev/mailbox\">Mailbox</a></nav>{body}</body></html>",
        escape(title)
    );
    (
        [(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'unsafe-inline'; img-src * data:; \
             frame-src 'self'; form-action 'self'",
        )],
        Html(html),
    )
        .into_response()
}

#[cfg(debug_assertions)]
const STYLE: &str = "body{font-family:system-ui,sans-serif;max-width:60rem;margin:2rem auto;padding:0 1rem}\
th{text-align:left;padding-right:1rem;vertical-align:top}iframe{width:100%;height:30rem;border:1px solid #ccc}\
pre{white-space:pre-wrap;background:#f6f6f6;padding:1rem}label{display:block;margin:.5rem 0}\
input,textarea{width:100%;font:inherit}textarea{height:10rem}";

#[cfg(debug_assertions)]
const MAILBOX_FORM: &str = "<h1>Deliver a test email</h1>\
<p>Sends a message to the app's mailbox (<code>ocre g mailbox</code>) through wrangler's local email endpoint, \
like Cloudflare Email Routing would.</p>\
<form id=\"compose\"><label>From <input name=\"from\" value=\"ada@example.com\" required></label>\
<label>To <input name=\"to\" value=\"support@example.com\" required></label>\
<label>Subject <input name=\"subject\" value=\"Hello\"></label>\
<label>Body <textarea name=\"body\">Hello from ocre dev.</textarea></label>\
<button>Deliver</button></form><pre id=\"result\"></pre><script src=\"/ocre/dev/mailbox.js\"></script>";

#[cfg(debug_assertions)]
const MAILBOX_JS: &str = r#"// Builds a raw message and posts it to wrangler's local email endpoint.
const encode = (text) => /^[\x20-\x7e]*$/.test(text)
  ? text
  : `=?UTF-8?B?${btoa(String.fromCharCode(...new TextEncoder().encode(text)))}?=`;
document.getElementById("compose").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.target;
  const raw = [
    `From: ${form.from.value}`,
    `To: ${form.to.value}`,
    `Subject: ${encode(form.subject.value)}`,
    `Message-ID: <${Date.now()}.${Math.random().toString(36).slice(2)}@ocre.dev>`,
    `Date: ${new Date().toUTCString()}`,
    "MIME-Version: 1.0",
    "Content-Type: text/plain; charset=utf-8",
    "Content-Transfer-Encoding: 8bit",
    "",
    form.body.value.replace(/\r?\n/g, "\r\n"),
  ].join("\r\n");
  const query = new URLSearchParams({ from: form.from.value, to: form.to.value });
  const response = await fetch(`/cdn-cgi/local/email?${query}`, { method: "POST", body: raw });
  document.getElementById("result").textContent = `${response.status}: ${await response.text()}`;
});
"#;

#[cfg(debug_assertions)]
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
#[path = "../../tests/mail/dev.rs"]
mod tests;
