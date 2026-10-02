//! Test helpers for Ocre apps, like Rails' `ActionDispatch::IntegrationTest`
//! and `ActiveSupport::Testing`: a request client for the running app, the
//! test database, the server log, time travel and assertions.
//!
//! Feature `testing`, native builds only: a generated app lists
//! `ocre = { ..., features = ["testing"] }` under `[dev-dependencies]`, so the
//! module is in `cargo test` and never in the WebAssembly build.
//!
//! # How request tests run
//!
//! Handlers need workerd (D1, KV, Queues... exist only there), so request
//! tests talk HTTP to the real runtime. `ocre test --e2e`:
//!
//! 1. creates a fresh local D1 database in `.wrangler/test-state` (the
//!    development data in `.wrangler/state` is untouched), applies the
//!    migrations and loads the fixtures of `tests/fixtures/`,
//! 2. starts one `cf dev` on that state for the whole run, logging to
//!    `.wrangler/test-state/dev.log`,
//! 3. runs `cargo test -- --ignored` with [`TEST_URL`], [`TEST_STATE`] and
//!    [`TEST_LOG`] set. In an Ocre app, `#[ignore]` marks the tests that need
//!    the runtime: plain `cargo test` (and `ocre test`) skips them.
//!
//! ```no_run
//! // tests/posts.rs
//! use ocre::testing::Client;
//!
//! #[test]
//! #[ignore = "request test: run with `ocre test --e2e`"]
//! fn creates_a_post() {
//!     let mut client = Client::new();
//!     let created = client.post("/posts", &[("title", "Hello"), ("body", "First post")]);
//!     created.assert_redirect_to("/posts/1");
//!     assert_eq!(client.flash("notice").as_deref(), Some("Post was successfully created."));
//!     client.follow_redirect(&created).assert_status(200).assert_contains("Hello");
//! }
//! ```
//!
//! There are no per-test transactions (Rails' transactional tests): the
//! database belongs to the `cf dev` process, which the test process reaches
//! only over HTTP or through wrangler. The database is fresh for each run;
//! tests running in parallel share it, so they create their own records
//! (factories give unique values) and assert on them rather than on global
//! counts, or run with `-- --test-threads=1`.

use std::{
    collections::BTreeMap,
    fmt::Debug,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use cookie::{Cookie, CookieJar, Key};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::{SESSION_COOKIE, mail::Email};

/// Environment variable holding the base URL of the server `ocre test --e2e` started.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::testing::TEST_URL, "OCRE_TEST_URL");
/// ```
pub const TEST_URL: &str = "OCRE_TEST_URL";

/// Environment variable holding the local state directory of the test run (`.wrangler/test-state`).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::testing::TEST_STATE, "OCRE_TEST_STATE");
/// ```
pub const TEST_STATE: &str = "OCRE_TEST_STATE";

/// Environment variable holding the path of the test server's log.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::testing::TEST_LOG, "OCRE_TEST_LOG");
/// ```
pub const TEST_LOG: &str = "OCRE_TEST_LOG";

/// How long [`eventually`] and [`Log::wait_for`] wait.
const WAIT: Duration = Duration::from_secs(30);

/// An HTTP client for request tests, with a cookie jar, like a browser tab
/// (Rails' integration session).
///
/// Every request carries `Sec-Fetch-Site: same-origin`, as a browser's
/// same-site form does, so Ocre's cross-site request check passes; use
/// [`Client::cross_site`] to test the check. Cookies from `Set-Cookie`
/// are sent back on the next requests, so a sign-in, the session and flash
/// messages carry over. Redirects are not followed automatically: assert on
/// them, then call [`Client::follow_redirect`]. Two clients are two
/// independent visitors (Rails' `open_session`).
///
/// # Examples
///
/// ```no_run
/// use ocre::testing::Client;
///
/// let mut client = Client::new();
/// client.get("/up").assert_status(200).assert_contains("OK");
/// ```
#[derive(Debug)]
pub struct Client {
    agent: ureq::Agent,
    base: String,
    headers: Vec<(String, String)>,
    /// `name` -> value as the server sent it (percent-encoded).
    cookies: BTreeMap<String, String>,
}

impl Client {
    /// A client for the server `ocre test --e2e` started ([`TEST_URL`]).
    ///
    /// # Panics
    ///
    /// When [`TEST_URL`] is not set: the test was run with plain `cargo test -- --ignored`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.get("/").assert_success();
    /// ```
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let base = std::env::var(TEST_URL).unwrap_or_else(|_| {
            panic!("{TEST_URL} is not set. Fix: run request tests with `ocre test --e2e`, which starts the server")
        });
        Self::with_base_url(&base)
    }

    /// A client for the app at `base` (`http://localhost:8787`), e.g. a running `ocre dev`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::with_base_url("http://localhost:8787");
    /// client.get("/up").assert_status(200);
    /// ```
    pub fn with_base_url(base: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .allow_non_standard_methods(true)
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        Self {
            agent,
            base: base.trim_end_matches('/').to_owned(),
            headers: vec![("sec-fetch-site".to_owned(), "same-origin".to_owned())],
            cookies: BTreeMap::new(),
        }
    }

    /// Sends `name: value` with every request (Rails' `headers:`); replaces
    /// an earlier value of the same header.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new().header("Accept-Language", "fr");
    /// client.get("/").assert_contains("Bienvenue");
    /// ```
    pub fn header(mut self, name: &str, value: &str) -> Self {
        let name = name.to_ascii_lowercase();
        self.headers.retain(|(existing, _)| *existing != name);
        self.headers.push((name, value.to_owned()));
        self
    }

    /// Requests as a form posted from another site would (`Sec-Fetch-Site: cross-site`):
    /// Ocre's CSRF protection answers 403 to unsafe methods.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new().cross_site();
    /// client.post("/posts", &[("title", "x")]).assert_status(403);
    /// ```
    pub fn cross_site(self) -> Self {
        self.header("Sec-Fetch-Site", "cross-site")
    }

    /// Requests as htmx does (`HX-Request: true`), Rails' `xhr: true`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new().htmx();
    /// client.get("/posts").assert_not_contains("<html");
    /// ```
    pub fn htmx(self) -> Self {
        self.header("HX-Request", "true")
    }

    /// `GET path`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// ocre::testing::Client::new().get("/posts?page=2").assert_status(200);
    /// ```
    pub fn get(&mut self, path: &str) -> Response {
        self.request("GET", path, None)
    }

    /// `POST path` with an `application/x-www-form-urlencoded` body, as an HTML form sends it.
    ///
    /// `form` is anything serde_urlencoded takes: a slice of `(name, value)`
    /// pairs, a factory's `form()`, a struct deriving `Serialize`, or `&()` for an empty body.
    ///
    /// # Panics
    ///
    /// When `form` is not a flat list of fields.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.post("/posts", &[("title", "Hello"), ("body", "World")]).assert_status(303);
    /// client.post("/posts/1/delete", &()).assert_redirect_to("/posts");
    /// ```
    pub fn post(&mut self, path: &str, form: &impl Serialize) -> Response {
        let body = serde_urlencoded::to_string(form).expect("form fields encode");
        self.request("POST", path, Some(("application/x-www-form-urlencoded", body.into_bytes())))
    }

    /// `POST path` with `value` as a JSON body.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// let created = client.post_json("/api/posts", &ocre::serde_json::json!({"title": "Hello"}));
    /// assert_eq!(created.assert_status(201).json::<ocre::serde_json::Value>()["title"], "Hello");
    /// ```
    pub fn post_json(&mut self, path: &str, value: &impl Serialize) -> Response {
        self.json("POST", path, value)
    }

    /// `PATCH path` with `value` as a JSON body.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.patch_json("/api/posts/1", &ocre::serde_json::json!({"title": "New"})).assert_status(200);
    /// ```
    pub fn patch_json(&mut self, path: &str, value: &impl Serialize) -> Response {
        self.json("PATCH", path, value)
    }

    /// `PUT path` with `value` as a JSON body.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.put_json("/api/settings", &ocre::serde_json::json!({"theme": "dark"})).assert_success();
    /// ```
    pub fn put_json(&mut self, path: &str, value: &impl Serialize) -> Response {
        self.json("PUT", path, value)
    }

    /// `DELETE path`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// ocre::testing::Client::new().delete("/api/posts/1").assert_status(204);
    /// ```
    pub fn delete(&mut self, path: &str) -> Response {
        self.request("DELETE", path, None)
    }

    fn json(&mut self, method: &str, path: &str, value: &impl Serialize) -> Response {
        let body = serde_json::to_vec(value).expect("the value serializes to JSON");
        self.request(method, path, Some(("application/json", body)))
    }

    /// Sends `method path` with an optional `(content type, body)`: any
    /// method, any body (multipart uploads, raw bytes).
    ///
    /// # Panics
    ///
    /// When the server cannot be reached.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.request("HEAD", "/up", None).assert_status(200);
    /// client.request("POST", "/api/raw", Some(("text/plain", b"bytes".to_vec()))).assert_success();
    /// ```
    pub fn request(&mut self, method: &str, path: &str, body: Option<(&str, Vec<u8>)>) -> Response {
        let url = format!("{}{path}", self.base);
        let mut builder = ureq::http::Request::builder().method(method).uri(&url);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if !self.cookies.is_empty() {
            let cookies: Vec<String> = self.cookies.iter().map(|(name, value)| format!("{name}={value}")).collect();
            builder = builder.header("cookie", cookies.join("; "));
        }
        let result = match body {
            Some((content_type, bytes)) => {
                self.agent.run(builder.header("content-type", content_type).body(bytes).expect("valid request"))
            }
            None => self.agent.run(builder.body(()).expect("valid request")),
        };
        let mut response = result.unwrap_or_else(|err| panic!("{method} {url} failed: {err}"));
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(name, value)| (name.as_str().to_owned(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
            .collect();
        for (name, value) in &headers {
            if name == "set-cookie" {
                self.store_cookie(value);
            }
        }
        let status = response.status().as_u16();
        let body = if method == "HEAD" || status == 204 || status == 304 {
            String::new()
        } else {
            let bytes = body_bytes(response.body_mut().read_to_vec(), method, &url);
            String::from_utf8_lossy(&bytes).into_owned()
        };
        Response { status, headers, body }
    }

    fn store_cookie(&mut self, set_cookie: &str) {
        let Ok(cookie) = Cookie::parse(set_cookie) else { return };
        let removed = cookie.value().is_empty() || cookie.max_age().is_some_and(|max_age| max_age.whole_seconds() <= 0);
        if removed {
            self.cookies.remove(cookie.name());
        } else {
            self.cookies.insert(cookie.name().to_owned(), cookie.value().to_owned());
        }
    }

    /// Follows the redirect `response` answered: `GET` of its `Location` (Rails' `follow_redirect!`).
    ///
    /// # Panics
    ///
    /// When `response` has no `Location` header.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// let created = client.post("/posts", &[("title", "Hello"), ("body", "World")]);
    /// client.follow_redirect(&created).assert_contains("Post was successfully created.");
    /// ```
    pub fn follow_redirect(&mut self, response: &Response) -> Response {
        let location = response.location().unwrap_or_else(|| {
            panic!("expected a redirect, got {} without a Location header:\n{}", response.status, response.excerpt())
        });
        let path = location.strip_prefix(&self.base).unwrap_or(location).to_owned();
        self.get(&path)
    }

    /// The cookie `name` as the server set it (percent-encoded), if the jar has it.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.get("/");
    /// assert!(client.cookie("_ocre_session").is_none());
    /// ```
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies.get(name).map(String::as_str)
    }

    /// Sets a cookie sent with the next requests, as if the server had set it.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.set_cookie("locale", "fr");
    /// ```
    pub fn set_cookie(&mut self, name: &str, value: &str) {
        self.cookies.insert(name.to_owned(), value.to_owned());
    }

    /// The session data (Rails' `session` in tests), decrypted with the
    /// app's `SECRET_KEY_BASE` (the environment variable, else `.dev.vars`):
    /// empty without a session cookie. Flash messages set for the next
    /// request are under `_flash`: [`Client::flash`] reads them.
    ///
    /// # Panics
    ///
    /// When there is a session cookie but no `SECRET_KEY_BASE`, or the
    /// cookie does not decrypt with it.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.post("/session", &[("email", "ada@example.com"), ("password", "secret")]);
    /// assert!(client.session().contains_key("user_id"));
    /// ```
    pub fn session(&self) -> Map<String, Value> {
        let Some(value) = self.cookies.get(SESSION_COOKIE) else { return Map::new() };
        let secret = var("SECRET_KEY_BASE")
            .expect("SECRET_KEY_BASE is not set (environment or .dev.vars): cannot read the session");
        decrypt_session(value, &secret).expect("the session cookie does not decrypt with SECRET_KEY_BASE")
    }

    /// The flash message of `kind` (`notice`, `alert`...) set by the last
    /// request for the next page (Rails' `flash[:notice]` after an action).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.post("/posts", &[("title", "Hello"), ("body", "World")]);
    /// assert_eq!(client.flash("notice").as_deref(), Some("Post was successfully created."));
    /// ```
    pub fn flash(&self, kind: &str) -> Option<String> {
        self.session().get("_flash")?.get(kind)?.as_str().map(str::to_owned)
    }

    /// Emails the app sent with `MAIL_ADAPTER = "log"` (Rails'
    /// `ActionMailer::Base.deliveries`), oldest first: the last 20, kept by
    /// the Worker instance. Reads `GET /ocre/dev/mailers/sent.json`, which
    /// exists when `routes()` merges `ocre::mail::dev_routes` (`ocre g mailer` adds it).
    ///
    /// # Panics
    ///
    /// When the endpoint does not answer a JSON list.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// let before = client.deliveries().len();
    /// client.post("/passwords", &[("email", "ada@example.com")]);
    /// let sent = client.deliveries();
    /// assert_eq!(sent.len(), before + 1);
    /// assert_eq!(sent.last().unwrap().to, ["ada@example.com"]);
    /// ```
    pub fn deliveries(&mut self) -> Vec<Email> {
        let response = self.get("/ocre/dev/mailers/sent.json");
        response.assert_status(200);
        let sent: Vec<Value> = response.json();
        sent.into_iter()
            .map(|entry| serde_json::from_value(entry["email"].clone()).expect("a captured email"))
            .collect()
    }

    /// Messages the app broadcast to realtime channels (Rails'
    /// `assert_broadcasts`), oldest first: the last 50, kept by the Worker
    /// instance. Reads `GET /ocre/dev/realtime/sent.json`, which exists when
    /// `routes()` merges `ocre::realtime::dev_routes()` (`ocre g scaffold ... --realtime` adds it).
    ///
    /// # Panics
    ///
    /// When the endpoint does not answer a JSON list.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.post("/posts", &[("title", "Live"), ("body", "b")]);
    /// let last = client.broadcasts().pop().unwrap();
    /// assert_eq!(last.channel, "posts");
    /// assert!(last.message.contains("Live"));
    /// ```
    pub fn broadcasts(&mut self) -> Vec<Broadcast> {
        let response = self.get("/ocre/dev/realtime/sent.json");
        response.assert_status(200);
        response.json()
    }

    /// Jobs the app enqueued and ran (Rails' `assert_enqueued_with`,
    /// `assert_performed_jobs`): the last 50 of each, oldest first, kept by
    /// the Worker instance. Reads `GET /ocre/dev/jobs.json`, which the first
    /// `ocre g job` merges into `routes()` (`ocre::jobs::dev_routes()`).
    /// Local queues deliver within a second or so: wait for a run with
    /// [`eventually`].
    ///
    /// # Panics
    ///
    /// When the endpoint does not answer the expected JSON.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ocre::testing::{Client, eventually};
    ///
    /// let mut client = Client::new();
    /// client.post("/signups", &[("email", "ada@example.com")]);
    /// let jobs = client.jobs();
    /// assert_eq!(jobs.enqueued.last().unwrap().name(), Some("send_welcome"));
    /// eventually(|| client.jobs().performed.iter().any(|run| run.job == "send_welcome" && run.outcome == "done").then_some(()));
    /// ```
    pub fn jobs(&mut self) -> Jobs {
        let response = self.get("/ocre/dev/jobs.json");
        response.assert_status(200);
        response.json()
    }

    /// Delivers an email to the app's mailbox (`ocre g mailbox`), as
    /// Cloudflare Email Routing would (Rails' `receive_inbound_email_from_mail`):
    /// a plain-text message posted to the local server's email endpoint
    /// (`POST /cdn-cgi/local/email?from=&to=`), which runs the Worker's
    /// `email` event. The response says whether the mailbox accepted it.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let mut client = ocre::testing::Client::new();
    /// client.receive_email("ada@example.com", "support@example.com", "Help", "My order is late.").assert_success();
    /// ```
    pub fn receive_email(&mut self, from: &str, to: &str, subject: &str, body: &str) -> Response {
        let query = serde_urlencoded::to_string([("from", from), ("to", to)]).expect("encoding two strings");
        let raw = raw_email(from, to, subject, body, sequence());
        self.request("POST", &format!("/cdn-cgi/local/email?{query}"), Some(("message/rfc822", raw.into_bytes())))
    }
}

/// A plain-text RFC 5322 message; a non-ASCII subject is RFC 2047 encoded.
fn raw_email(from: &str, to: &str, subject: &str, body: &str, n: u64) -> String {
    let printable = subject.bytes().all(|byte| (0x20..0x7f).contains(&byte));
    let subject = if printable {
        subject.to_owned()
    } else {
        use base64::Engine as _;
        format!("=?UTF-8?B?{}?=", base64::engine::general_purpose::STANDARD.encode(subject))
    };
    let body = body.replace("\r\n", "\n").replace('\n', "\r\n");
    format!(
        "From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nMessage-ID: <{n}.{}@ocre.test>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body}",
        crate::now()
    )
}

/// A message broadcast to a realtime channel, from [`Client::broadcasts`].
///
/// # Examples
///
/// ```
/// let broadcast: ocre::testing::Broadcast =
///     ocre::serde_json::from_str(r#"{"id":1,"channel":"posts","message":"<li>Hi</li>"}"#).unwrap();
/// assert_eq!(broadcast.channel, "posts");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Broadcast {
    /// Position in the capture, from 1.
    pub id: u64,
    /// The channel it went to (`posts`).
    pub channel: String,
    /// The message: HTML or JSON text.
    pub message: String,
}

/// Jobs the app enqueued and ran, from [`Client::jobs`].
///
/// # Examples
///
/// ```
/// let jobs: ocre::testing::Jobs = ocre::serde_json::from_str(
///     r#"{"enqueued":[{"id":1,"queue":"default","job":{"send_welcome":{"user_id":7}}}],
///         "performed":[{"id":2,"job":"send_welcome","outcome":"done"}]}"#,
/// )
/// .unwrap();
/// assert_eq!(jobs.enqueued[0].name(), Some("send_welcome"));
/// assert_eq!(jobs.performed[0].outcome, "done");
/// ```
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct Jobs {
    /// Jobs sent to a queue, oldest first.
    pub enqueued: Vec<EnqueuedJob>,
    /// Jobs the queue consumer ran, oldest first.
    pub performed: Vec<PerformedJob>,
}

/// A job sent to a queue, from [`Client::jobs`].
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct EnqueuedJob {
    /// Position in the capture, from 1 (shared with runs).
    pub id: u64,
    /// The queue: `default`, or the name given to `ocre::jobs::queue`.
    pub queue: String,
    /// The job as the app serialized it: `{"send_welcome": {"user_id": 7}}`.
    pub job: Value,
}

impl EnqueuedJob {
    /// The job's name: the key of its JSON object (`send_welcome`), as [`PerformedJob::job`] names it.
    ///
    /// # Examples
    ///
    /// ```
    /// let job: ocre::testing::EnqueuedJob =
    ///     ocre::serde_json::from_str(r#"{"id":1,"queue":"default","job":"cleanup"}"#).unwrap();
    /// assert_eq!(job.name(), Some("cleanup"));
    /// ```
    pub fn name(&self) -> Option<&str> {
        match &self.job {
            Value::Object(map) => map.keys().next().map(String::as_str),
            Value::String(name) => Some(name),
            _ => None,
        }
    }
}

/// A job run by the queue consumer, from [`Client::jobs`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PerformedJob {
    /// Position in the capture, from 1 (shared with enqueued jobs).
    pub id: u64,
    /// The job's name (`send_welcome`), or `mail` for `deliver_later` emails.
    pub job: String,
    /// `done`, `discarded` (an error a retry cannot fix) or `retried`.
    pub outcome: String,
}

/// Runs a future to completion on the test thread: `async` handlers,
/// helpers and `ocre::password` in plain unit tests (no async runtime runs
/// outside workerd). It is [`pollster::block_on`].
///
/// # Examples
///
/// ```
/// async fn up() -> &'static str {
///     "OK"
/// }
///
/// assert_eq!(ocre::testing::block_on(up()), "OK");
/// ```
pub use pollster::block_on;

/// A variable of the app under test: the environment variable `name`, else
/// its value in `.dev.vars` (the file `ocre dev` loads), as the Worker sees
/// it. Request tests use it for the secrets they sign with (a webhook's).
///
/// # Examples
///
/// ```no_run
/// let secret = ocre::testing::var("PAYMENTS_WEBHOOK_SECRET").expect("in .dev.vars");
/// # let _ = secret;
/// ```
pub fn var(name: &str) -> Option<String> {
    dev_var(name, std::env::var(name).ok(), Path::new(".dev.vars"))
}

/// `name` from the environment (`env`), else from the `dotenv` file.
fn dev_var(name: &str, env: Option<String>, dotenv: &Path) -> Option<String> {
    if env.is_some() {
        return env;
    }
    let text = std::fs::read_to_string(dotenv).ok()?;
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix(name)?.trim_start().strip_prefix('=')?.trim();
        Some(value.trim_matches('"').to_owned())
    })
}

/// The session data in the cookie `value` (percent-encoded), `None` when it does not decrypt.
fn decrypt_session(value: &str, secret: &str) -> Option<Map<String, Value>> {
    let cookie = Cookie::parse_encoded(format!("{SESSION_COOKIE}={value}")).ok()?;
    let mut jar = CookieJar::new();
    jar.add_original(cookie.into_owned());
    let decrypted = jar.private(&Key::derive_from(secret.as_bytes())).get(SESSION_COOKIE)?;
    serde_json::from_str(decrypted.value()).ok()
}

/// A response in a request test, with chainable assertions (Rails'
/// `assert_response`, `assert_redirected_to`, `assert_match`).
///
/// Assertions panic with the status and the start of the body, and return
/// the response so they chain.
///
/// # Examples
///
/// ```no_run
/// let mut client = ocre::testing::Client::new();
/// client.get("/posts").assert_status(200).assert_contains("<h1>Posts</h1>");
/// ```
#[derive(Debug, Clone)]
pub struct Response {
    /// HTTP status code.
    pub status: u16,
    /// Headers in the order received, names lowercase.
    pub headers: Vec<(String, String)>,
    /// Body as text (invalid UTF-8 replaced); empty for `HEAD`, 204 and 304.
    pub body: String,
}

impl Response {
    /// The first value of the header `name` (any case).
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::testing::Response { status: 200, headers: vec![("content-type".into(), "text/html".into())], body: String::new() };
    /// assert_eq!(response.header("Content-Type"), Some("text/html"));
    /// ```
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }

    /// The `Location` header of a redirect.
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::testing::Response { status: 303, headers: vec![("location".into(), "/posts/1".into())], body: String::new() };
    /// assert_eq!(response.location(), Some("/posts/1"));
    /// ```
    pub fn location(&self) -> Option<&str> {
        self.header("location")
    }

    /// The body parsed as JSON (Rails' `response.parsed_body`).
    ///
    /// # Panics
    ///
    /// When the body is not JSON of type `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::testing::Response { status: 200, headers: vec![], body: r#"{"id":1}"#.into() };
    /// assert_eq!(response.json::<ocre::serde_json::Value>()["id"], 1);
    /// ```
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|err| panic!("the body is not the expected JSON ({err}):\n{}", self.excerpt()))
    }

    /// Asserts the status code (Rails' `assert_response 201`).
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::testing::Response { status: 404, headers: vec![], body: String::new() };
    /// response.assert_status(404);
    /// ```
    #[track_caller]
    pub fn assert_status(&self, status: u16) -> &Self {
        assert!(self.status == status, "expected status {status}, got {}:\n{}", self.status, self.excerpt());
        self
    }

    /// Asserts a 2xx status (Rails' `assert_response :success`).
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::testing::Response { status: 204, headers: vec![], body: String::new() };
    /// response.assert_success();
    /// ```
    #[track_caller]
    pub fn assert_success(&self) -> &Self {
        assert!((200..300).contains(&self.status), "expected a 2xx status, got {}:\n{}", self.status, self.excerpt());
        self
    }

    /// Asserts a 3xx redirect to `location` (Rails' `assert_redirected_to`).
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::testing::Response { status: 303, headers: vec![("location".into(), "/posts/1".into())], body: String::new() };
    /// response.assert_redirect_to("/posts/1");
    /// ```
    #[track_caller]
    pub fn assert_redirect_to(&self, location: &str) -> &Self {
        assert!(
            (300..400).contains(&self.status) && self.location() == Some(location),
            "expected a redirect to {location}, got {} to {:?}:\n{}",
            self.status,
            self.location(),
            self.excerpt()
        );
        self
    }

    /// Asserts the body contains `text` (HTML-escaped as templates escape it: `'` is `&#39;`).
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::testing::Response { status: 200, headers: vec![], body: "<h1>Posts</h1>".into() };
    /// response.assert_contains("<h1>Posts</h1>");
    /// ```
    #[track_caller]
    pub fn assert_contains(&self, text: &str) -> &Self {
        assert!(self.body.contains(text), "expected the body to contain {text:?}:\n{}", self.excerpt());
        self
    }

    /// Asserts the body does not contain `text`.
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::testing::Response { status: 200, headers: vec![], body: "<h1>Posts</h1>".into() };
    /// response.assert_not_contains("Error");
    /// ```
    #[track_caller]
    pub fn assert_not_contains(&self, text: &str) -> &Self {
        assert!(!self.body.contains(text), "expected the body not to contain {text:?}:\n{}", self.excerpt());
        self
    }

    /// Asserts the header `name` has `value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let response = ocre::testing::Response { status: 200, headers: vec![("content-type".into(), "application/json".into())], body: String::new() };
    /// response.assert_header("Content-Type", "application/json");
    /// ```
    #[track_caller]
    pub fn assert_header(&self, name: &str, value: &str) -> &Self {
        assert!(
            self.header(name) == Some(value),
            "expected header {name}: {value}, got {:?}:\n{}",
            self.header(name),
            self.excerpt()
        );
        self
    }

    /// Status line and the first 2,000 characters of the body, for failure messages.
    fn excerpt(&self) -> String {
        let body: String = self.body.chars().take(2000).collect();
        format!("HTTP {}\n{body}", self.status)
    }
}

/// Rows returned by `query`, run on the test database (Rails' `ActiveRecord::Base.connection.select_all`).
///
/// It runs the app's wrangler (`node_modules/.bin/wrangler d1 execute DB
/// --local`) on [`TEST_STATE`]: about a second per call, so keep it to
/// setup and checks. Several statements may be separated by `;`; the rows
/// of the last one are returned. Values are SQL literals: build them with
/// [`quote`].
///
/// # Panics
///
/// When wrangler fails (the message has its output), e.g. on a SQL error.
///
/// # Examples
///
/// ```no_run
/// use ocre::testing::{quote, sql};
///
/// let rows = sql(&format!("SELECT id FROM posts WHERE title = {}", quote("Hello")));
/// assert_eq!(rows.len(), 1);
/// ```
pub fn sql(query: &str) -> Vec<Map<String, Value>> {
    sql_in(&app_root(), &state_dir(), query)
}

thread_local! {
    /// The app's directory: `cargo test` runs tests in the package's directory;
    /// Ocre's own unit tests point it at a scratch app.
    static APP_ROOT: std::cell::RefCell<PathBuf> = std::cell::RefCell::new(PathBuf::from("."));
}

fn app_root() -> PathBuf {
    APP_ROOT.with(|root| root.borrow().clone())
}

fn state_dir() -> String {
    std::env::var(TEST_STATE).unwrap_or_else(|_| ".wrangler/test-state".to_owned())
}

fn sql_in(root: &Path, state: &str, query: &str) -> Vec<Map<String, Value>> {
    let output = Command::new(root.join("node_modules/.bin/wrangler"))
        .args(["d1", "execute", "DB", "--local", "--json", "--command", query])
        .args(["-c", ".wrangler/ocre-d1.json", "--persist-to", state])
        .current_dir(root)
        .output()
        .unwrap_or_else(|err| panic!("could not run node_modules/.bin/wrangler ({err}). Fix: run `npm install`"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "SQL failed: {query}\n{stdout}{}\nFix: run the tests with `ocre test --e2e`, which creates the test database",
        String::from_utf8_lossy(&output.stderr)
    );
    let statements: Vec<Value> =
        serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("unexpected wrangler output ({err}):\n{stdout}"));
    let rows = statements.last().and_then(|statement| statement["results"].as_array()).cloned().unwrap_or_default();
    rows.into_iter().filter_map(|row| if let Value::Object(row) = row { Some(row) } else { None }).collect()
}

/// `value` as a SQL literal: strings quoted (`'` doubled), numbers as is,
/// booleans as `1`/`0`, null as `NULL`, arrays and objects as JSON text.
///
/// # Examples
///
/// ```
/// use ocre::{serde_json::json, testing::quote};
///
/// assert_eq!(quote("it's"), "'it''s'");
/// assert_eq!(quote(42), "42");
/// assert_eq!(quote(true), "1");
/// assert_eq!(quote(json!({"a": 1})), r#"'{"a":1}'"#);
/// assert_eq!(quote(None::<i64>), "NULL");
/// ```
pub fn quote(value: impl Into<Value>) -> String {
    quote_value(&value.into())
}

fn quote_value(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_owned(),
        Value::Bool(true) => "1".to_owned(),
        Value::Bool(false) => "0".to_owned(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => format!("'{}'", text.replace('\'', "''")),
        other => format!("'{}'", other.to_string().replace('\'', "''")),
    }
}

/// Inserts a row into `table` of the test database and returns its `id`:
/// what generated factories (`tests/factories/`) call. One wrangler call (about a second).
///
/// # Panics
///
/// Like [`sql`], e.g. on a constraint violation.
///
/// # Examples
///
/// ```no_run
/// use ocre::serde_json::json;
///
/// let id = ocre::testing::insert("posts", &[("title", json!("Hello")), ("published", json!(true))]);
/// assert!(id > 0);
/// ```
pub fn insert(table: &str, values: &[(&str, Value)]) -> i64 {
    sql(&insert_sql(table, values))
        .first()
        .and_then(|row| row.get("id"))
        .and_then(Value::as_i64)
        .expect("INSERT ... RETURNING id returns the id")
}

fn insert_sql(table: &str, values: &[(&str, Value)]) -> String {
    let columns: Vec<&str> = values.iter().map(|(column, _)| *column).collect();
    let literals: Vec<String> = values.iter().map(|(_, value)| quote_value(value)).collect();
    format!("INSERT INTO {table} ({}) VALUES ({}) RETURNING id", columns.join(", "), literals.join(", "))
}

/// Number of rows of `table` in the test database, for [`assert_difference`].
///
/// # Examples
///
/// ```no_run
/// let posts = ocre::testing::count("posts");
/// ```
pub fn count(table: &str) -> i64 {
    sql(&format!("SELECT COUNT(*) AS count FROM {table}"))[0]["count"].as_i64().expect("COUNT(*) is an integer")
}

/// The `id` of the fixture labelled `label` (Rails' `users(:david).id`):
/// the loader of `tests/fixtures/*.yml` gives a record without an explicit
/// `id` the CRC-32 of its label modulo 2^30 - 1, like Rails.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::testing::fixture_id("david"), 127326141);
/// ```
pub fn fixture_id(label: &str) -> i64 {
    i64::from(crc32(label.as_bytes()) % ((1 << 30) - 1))
}

/// The row of `table` loaded from the fixture `label` (Rails' `posts(:first)`).
///
/// # Panics
///
/// When there is no such row.
///
/// # Examples
///
/// ```no_run
/// let post = ocre::testing::fixture("posts", "first");
/// assert_eq!(post["title"], "Hello");
/// ```
pub fn fixture(table: &str, label: &str) -> Map<String, Value> {
    let rows = sql(&format!("SELECT * FROM {table} WHERE id = {}", fixture_id(label)));
    rows.into_iter().next().unwrap_or_else(|| panic!("no fixture {label} in {table} (tests/fixtures/{table}.yml)"))
}

/// CRC-32 (IEEE, as zlib), bit by bit: fixture labels are short.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A unique number per call in the test process, for unique test data
/// (FactoryBot's `sequence`): 1, 2, 3...
///
/// # Examples
///
/// ```
/// let (a, b) = (ocre::testing::sequence(), ocre::testing::sequence());
/// assert!(b > a);
/// ```
pub fn sequence() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The test server's log (`cf dev` output: `console.log`, job and cron
/// lines, errors), read from the position of [`Log::mark`] on.
///
/// Jobs and crons log `[ocre jobs] <name> done` and failures; errors
/// answered 500 log `[ocre] ...`. Waiting for those lines replaces Rails'
/// `assert_performed_jobs` and `assert_error_reported`: queues deliver
/// asynchronously, as in production.
///
/// # Examples
///
/// ```no_run
/// use ocre::testing::{Client, Log};
///
/// let log = Log::mark();
/// Client::new().post("/signups", &[("email", "ada@example.com")]);
/// log.wait_for("[ocre jobs] send_welcome done");
/// ```
#[derive(Debug, Clone)]
pub struct Log {
    path: PathBuf,
    start: usize,
}

impl Log {
    /// The log from now on ([`TEST_LOG`], else `.wrangler/test-state/dev.log`).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let log = ocre::testing::Log::mark();
    /// assert!(log.text().is_empty());
    /// ```
    pub fn mark() -> Self {
        let path =
            std::env::var(TEST_LOG).map_or_else(|_| PathBuf::from(".wrangler/test-state/dev.log"), PathBuf::from);
        Self::mark_at(path)
    }

    fn mark_at(path: PathBuf) -> Self {
        let start = std::fs::metadata(&path).map_or(0, |meta| usize::try_from(meta.len()).unwrap_or(usize::MAX));
        Self { path, start }
    }

    /// What was logged since the mark.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let log = ocre::testing::Log::mark();
    /// ocre::testing::Client::new().get("/boom");
    /// assert!(log.text().contains("[ocre]"));
    /// ```
    pub fn text(&self) -> String {
        let bytes = std::fs::read(&self.path).unwrap_or_default();
        String::from_utf8_lossy(bytes.get(self.start..).unwrap_or_default()).into_owned()
    }

    /// Waits up to 30 seconds for a line containing `needle` and returns it.
    ///
    /// # Panics
    ///
    /// When no such line arrives in time; the message has the log since the mark.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let log = ocre::testing::Log::mark();
    /// log.wait_for("[ocre jobs] send_welcome done");
    /// ```
    #[track_caller]
    pub fn wait_for(&self, needle: &str) -> String {
        self.wait_for_within(needle, WAIT)
    }

    #[track_caller]
    fn wait_for_within(&self, needle: &str, timeout: Duration) -> String {
        let found = poll(timeout, || self.text().lines().find(|line| line.contains(needle)).map(str::to_owned));
        found.unwrap_or_else(|| panic!("no log line containing {needle:?} within {timeout:?}; log:\n{}", self.text()))
    }
}

/// Retries `check` every 100 ms for up to 30 seconds until it returns
/// `Some`, for effects that happen later: a queued job, a cron, a
/// broadcast.
///
/// # Panics
///
/// When `check` still returns `None` after 30 seconds.
///
/// # Examples
///
/// ```
/// let mut tries = 0;
/// let value = ocre::testing::eventually(|| {
///     tries += 1;
///     (tries == 3).then_some("done")
/// });
/// assert_eq!(value, "done");
/// ```
#[track_caller]
pub fn eventually<T>(check: impl FnMut() -> Option<T>) -> T {
    eventually_within(WAIT, check)
}

#[track_caller]
fn eventually_within<T>(timeout: Duration, check: impl FnMut() -> Option<T>) -> T {
    poll(timeout, check).unwrap_or_else(|| panic!("the condition did not hold within {timeout:?}"))
}

/// The body of a response, or a panic naming the request.
fn body_bytes(bytes: Result<Vec<u8>, ureq::Error>, method: &str, url: &str) -> Vec<u8> {
    bytes.unwrap_or_else(|err| panic!("{method} {url}: body: {err}"))
}

fn poll<T>(timeout: Duration, mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Asserts that `block` changes the number `expression` returns by
/// `difference` (Rails' `assert_difference`), and returns the block's value.
///
/// # Examples
///
/// ```
/// let count = std::cell::Cell::new(1);
/// ocre::testing::assert_difference(|| count.get(), 1, || count.set(2));
/// ```
#[track_caller]
pub fn assert_difference<R>(mut expression: impl FnMut() -> i64, difference: i64, block: impl FnOnce() -> R) -> R {
    let before = expression();
    let result = block();
    let after = expression();
    assert!(
        after - before == difference,
        "expected a difference of {difference}, got {} ({before} -> {after})",
        after - before
    );
    result
}

/// Asserts that `block` leaves the number `expression` returns unchanged (Rails' `assert_no_difference`).
///
/// # Examples
///
/// ```
/// ocre::testing::assert_no_difference(|| 3, || ());
/// ```
#[track_caller]
pub fn assert_no_difference<R>(expression: impl FnMut() -> i64, block: impl FnOnce() -> R) -> R {
    assert_difference(expression, 0, block)
}

/// Asserts that `block` changes what `expression` returns (Rails'
/// `assert_changes`), and returns `(before, after)`.
///
/// # Examples
///
/// ```
/// let title = std::cell::RefCell::new("Draft".to_owned());
/// let (before, after) = ocre::testing::assert_changes(|| title.borrow().clone(), || *title.borrow_mut() = "Final".into());
/// assert_eq!((before.as_str(), after.as_str()), ("Draft", "Final"));
/// ```
#[track_caller]
pub fn assert_changes<T: PartialEq + Debug>(mut expression: impl FnMut() -> T, block: impl FnOnce()) -> (T, T) {
    let before = expression();
    block();
    let after = expression();
    assert!(before != after, "expected a change, still {after:?}");
    (before, after)
}

/// Asserts that `block` leaves what `expression` returns unchanged (Rails' `assert_no_changes`).
///
/// # Examples
///
/// ```
/// ocre::testing::assert_no_changes(|| "same", || ());
/// ```
#[track_caller]
pub fn assert_no_changes<T: PartialEq + Debug>(mut expression: impl FnMut() -> T, block: impl FnOnce()) {
    let before = expression();
    block();
    let after = expression();
    assert!(before == after, "expected no change, {before:?} became {after:?}");
}

/// Makes [`crate::now`] return `unix` on this thread until [`travel_back`]
/// (Rails' `travel_to`, frozen). Each Rust test runs on its own thread, so
/// other tests keep the real clock. Affects code running in the test
/// process (models, helpers, JWT), not the `cf dev` server.
///
/// # Examples
///
/// ```
/// ocre::testing::travel_to(1_767_225_600); // 2026-01-01T00:00:00Z
/// assert_eq!(ocre::now(), 1_767_225_600);
/// ocre::testing::travel_back();
/// ```
pub fn travel_to(unix: i64) {
    crate::clock::set_frozen(Some(unix));
}

/// Moves [`crate::now`] by `seconds` from its current value and freezes it there (Rails' `travel 1.day`).
///
/// # Examples
///
/// ```
/// ocre::testing::travel_to(1_000);
/// ocre::testing::travel(3_600);
/// assert_eq!(ocre::now(), 4_600);
/// ocre::testing::travel_back();
/// ```
pub fn travel(seconds: i64) {
    travel_to(crate::now() + seconds);
}

/// Stops [`crate::now`] at the current second (Rails' `freeze_time`); returns it.
///
/// # Examples
///
/// ```
/// let frozen = ocre::testing::freeze_time();
/// assert_eq!(ocre::now(), frozen);
/// ocre::testing::travel_back();
/// ```
pub fn freeze_time() -> i64 {
    let now = crate::now();
    travel_to(now);
    now
}

/// Returns [`crate::now`] to the real clock (Rails' `travel_back`).
///
/// # Examples
///
/// ```
/// ocre::testing::travel_to(0);
/// ocre::testing::travel_back();
/// assert!(ocre::now() > 0);
/// ```
pub fn travel_back() {
    crate::clock::set_frozen(None);
}

/// Replaces values that change on every run by placeholders, so a snapshot
/// or an `assert_eq!` on a whole page or JSON body is stable (Loco's
/// `cleanup_*` filters): UUIDs become `<UUID>`, ISO 8601 dates and times
/// (`2026-01-01`, `2026-01-01T12:00:00Z`, `2026-01-01 12:00:00`) become
/// `<DATE>`, and runs of 32 or more hexadecimal or base64url characters
/// (tokens, digests) become `<TOKEN>`.
///
/// # Examples
///
/// ```
/// let body = r#"{"id":"123e4567-e89b-42d3-a456-426614174000","at":"2026-09-29T10:00:00Z"}"#;
/// assert_eq!(ocre::testing::redact(body), r#"{"id":"<UUID>","at":"<DATE>"}"#);
/// ```
pub fn redact(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !is_token_byte(bytes[i - 1]);
        if boundary && let Some((len, placeholder)) = sensitive_at(&bytes[i..]) {
            out.push_str(placeholder);
            i += len;
            continue;
        }
        let ch = text[i..].chars().next().expect("i is on a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// Length and placeholder of the value starting `bytes`, if it is one to redact.
fn sensitive_at(bytes: &[u8]) -> Option<(usize, &'static str)> {
    if let Some(len) = uuid_len(bytes) {
        return Some((len, "<UUID>"));
    }
    if let Some(len) = date_len(bytes) {
        return Some((len, "<DATE>"));
    }
    let token = bytes.iter().take_while(|byte| is_token_byte(**byte)).count();
    let digits_or_letters =
        bytes[..token].iter().any(u8::is_ascii_digit) && bytes[..token].iter().any(u8::is_ascii_alphabetic);
    (token >= 32 && digits_or_letters).then_some((token, "<TOKEN>"))
}

/// Matches `pattern` (`9` a digit, `x` a hex digit, others literal) at the start of `bytes`.
fn matches(bytes: &[u8], pattern: &[u8]) -> bool {
    bytes.len() >= pattern.len()
        && pattern.iter().zip(bytes).all(|(want, got)| match want {
            b'9' => got.is_ascii_digit(),
            b'x' => got.is_ascii_hexdigit(),
            literal => literal == got,
        })
}

fn uuid_len(bytes: &[u8]) -> Option<usize> {
    const UUID: &[u8] = b"xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx";
    (matches(bytes, UUID) && bytes.get(UUID.len()).is_none_or(|byte| !is_token_byte(*byte))).then_some(UUID.len())
}

fn date_len(bytes: &[u8]) -> Option<usize> {
    if !matches(bytes, b"9999-99-99") {
        return None;
    }
    let mut len = 10;
    if matches(&bytes[len..], b"T99:99") || matches(&bytes[len..], b" 99:99") {
        len += 6;
        if matches(&bytes[len..], b":99") {
            len += 3;
        }
        if bytes.get(len) == Some(&b'.') {
            len += 1 + bytes[len + 1..].iter().take_while(|byte| byte.is_ascii_digit()).count();
        }
        if bytes.get(len) == Some(&b'Z') {
            len += 1;
        } else if matches(&bytes[len..], b"+99:99") || matches(&bytes[len..], b"-99:99") {
            len += 6;
        }
    }
    Some(len)
}

#[cfg(test)]
#[path = "../tests/testing.rs"]
mod tests;
