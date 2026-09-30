//! Error reporting (Rails' `Rails.error`): report errors with context to the logs and to services such as Sentry.
//!
//! Every [`Ctx`](crate::Ctx) has a [`Reporter`], [`Ctx::errors`](crate::Ctx::errors).
//! A report is logged at once (as an `error`, `warn` or `info` line with its
//! context) and handed to each [`Subscriber`] the app registered with
//! [`subscribe`]. Ocre reports on its own, with `handled: false`:
//!
//! | Source | When |
//! |---|---|
//! | `ocre.request` | a handler returned [`Error::Internal`](crate::Error::Internal) (a 500) |
//! | `ocre.job` | a background job failed (it is retried or discarded) |
//! | `ocre.cron` | a scheduled task failed |
//! | `ocre.mailbox` | the inbound email handler failed |
//!
//! ```no_run
//! use axum::extract::State;
//! use ocre::{Ctx, Result, errors::{Options, Severity}};
//!
//! async fn sync_orders(State(ctx): State<Ctx>) -> Result<&'static str> {
//!     ctx.errors().set_context("tenant", "acme"); // added to every report of this request
//!
//!     // Rails.error.handle: report and go on with a fallback.
//!     let rates: Vec<f64> = ctx.errors().handle(fetch_rates().await).unwrap_or_default();
//!
//!     // Rails.error.record: report, then fail the request.
//!     ctx.errors().record(charge().await)?;
//!
//!     // Rails.error.report, with options.
//!     if rates.is_empty() {
//!         let options = Options::new().severity(Severity::Info).context("provider", "ecb").source("rates");
//!         ctx.errors().report(&"no exchange rates today", options);
//!     }
//!     Ok("OK")
//! }
//!
//! async fn fetch_rates() -> Result<Vec<f64>> {
//!     Ok(vec![1.08])
//! }
//! async fn charge() -> Result<()> {
//!     Ok(())
//! }
//! # let _ = sync_orders;
//! ```
//!
//! # Subscribers
//!
//! Register them once per Worker instance, in the `start` event of
//! `src/lib.rs` (Ocre's initializer):
//!
//! ```no_run
//! #[worker::event(start)]
//! fn start() {
//!     ocre::errors::subscribe(ocre::errors::Sentry);
//! }
//! ```
//!
//! [`Sentry`] sends each report to the project of the `SENTRY_DSN` secret
//! (sentry.io or any Sentry-compatible service, such as GlitchTip or
//! Bugsink). Other services implement [`Subscriber`]: it turns a [`Report`]
//! into the HTTP request to send ([`Delivery`]), and Ocre sends it with
//! `fetch` once the response is ready.
//!
//! # Free plan
//!
//! Reports cost nothing until a subscriber sends one: then each delivery is
//! one subrequest (50 per request on the free plan), made after the
//! handler, before the response goes out, so an error response waits for
//! it. Only reports that happen are sent; a request without errors makes no
//! subrequest.

use std::{
    fmt::Display,
    sync::{Arc, Mutex, PoisonError},
};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{config::Environment, log::Logger};

/// How bad a report is (Rails' `severity:`).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::errors::Severity::Warning.as_str(), "warning");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// A failure: the default of [`Reporter::record`] and of Ocre's own reports.
    Error,
    /// Handled, worth looking at: the default of [`Reporter::report`] and [`Reporter::handle`].
    Warning,
    /// For information.
    Info,
}

impl Severity {
    /// `error`, `warning` or `info`, as Sentry names levels.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::errors::Severity::Error.as_str(), "error");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }

    fn level(self) -> crate::log::Level {
        match self {
            Self::Error => crate::log::Level::Error,
            Self::Warning => crate::log::Level::Warn,
            Self::Info => crate::log::Level::Info,
        }
    }
}

/// Options of one report (Rails' `handled:`, `severity:`, `context:`, `source:`).
///
/// Defaults: handled, [`Severity::Warning`], no context, source `application`.
///
/// # Examples
///
/// ```
/// use ocre::errors::{Options, Severity};
///
/// let options = Options::new().handled(false).severity(Severity::Error).context("order_id", 42).source("billing");
/// # let _ = options;
/// ```
#[derive(Debug, Clone)]
pub struct Options {
    handled: bool,
    severity: Severity,
    context: Map<String, Value>,
    source: String,
    except: Vec<&'static str>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            handled: true,
            severity: Severity::Warning,
            context: Map::new(),
            source: "application".to_owned(),
            except: Vec::new(),
        }
    }
}

impl Options {
    /// The defaults: handled, warning, no context, source `application`.
    ///
    /// # Examples
    ///
    /// ```
    /// let _ = ocre::errors::Options::new();
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the app recovered from the error (`false` for errors that failed the request or job).
    ///
    /// # Examples
    ///
    /// ```
    /// let _ = ocre::errors::Options::new().handled(false);
    /// ```
    pub fn handled(mut self, handled: bool) -> Self {
        self.handled = handled;
        self
    }

    /// The report's [`Severity`].
    ///
    /// # Examples
    ///
    /// ```
    /// let _ = ocre::errors::Options::new().severity(ocre::errors::Severity::Info);
    /// ```
    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Adds a context entry, merged over the request's [`Reporter::set_context`] entries.
    ///
    /// A value that does not serialize is `null`.
    ///
    /// # Examples
    ///
    /// ```
    /// let _ = ocre::errors::Options::new().context("user_id", 7);
    /// ```
    pub fn context(mut self, key: &str, value: impl Serialize) -> Self {
        self.context.insert(key.to_owned(), serde_json::to_value(value).unwrap_or(Value::Null));
        self
    }

    /// Where the error comes from, e.g. `billing` (Rails' `source:`).
    ///
    /// # Examples
    ///
    /// ```
    /// let _ = ocre::errors::Options::new().source("webhooks");
    /// ```
    pub fn source(mut self, source: &str) -> Self {
        self.source = source.to_owned();
        self
    }

    /// Does not send this report to the subscriber named `name` (Rails' `Rails.error.disable`).
    ///
    /// For errors a subscriber should not see, such as the failure of its
    /// own service. The report is still logged.
    ///
    /// # Examples
    ///
    /// ```
    /// let _ = ocre::errors::Options::new().except("sentry");
    /// ```
    pub fn except(mut self, subscriber: &'static str) -> Self {
        self.except.push(subscriber);
        self
    }
}

/// One reported error, as subscribers receive it.
///
/// # Examples
///
/// ```
/// use ocre::errors::{Options, Reporter};
///
/// let reporter = Reporter::default();
/// reporter.report(&"quota exceeded", Options::new().context("plan", "free"));
/// let report = &reporter.take()[0];
/// assert_eq!((report.message.as_str(), report.class.as_str()), ("quota exceeded", "&str"));
/// assert_eq!(report.context["plan"], "free");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// The error's type, e.g. `ocre::error::Error` (Sentry's exception type).
    pub class: String,
    /// The error's text (`Display`).
    pub message: String,
    /// Whether the app recovered from it.
    pub handled: bool,
    /// How bad it is.
    pub severity: Severity,
    /// The request's context, then the report's own entries.
    pub context: Map<String, Value>,
    /// Where it comes from: `application`, `ocre.request`, `ocre.job`...
    pub source: String,
    /// When it was reported, in Unix seconds.
    pub timestamp: i64,
    /// Development or production, from the build profile.
    pub environment: Environment,
    except: Vec<&'static str>,
}

/// An HTTP `POST` a [`Subscriber`] asks Ocre to send for a report.
///
/// # Examples
///
/// ```
/// let delivery = ocre::errors::Delivery {
///     url: "https://errors.example/api/reports".into(),
///     headers: vec![("content-type".into(), "application/json".into())],
///     body: "{}".into(),
/// };
/// # let _ = delivery;
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    /// Where to `POST`.
    pub url: String,
    /// Request headers, e.g. an API key.
    pub headers: Vec<(String, String)>,
    /// The request body.
    pub body: String,
}

/// An error-reporting service (Rails' error subscribers): turns a [`Report`] into the request to send.
///
/// `vars` looks up a Worker variable or secret by name (the service's key).
/// Return `None` to send nothing, e.g. when the key is not set. Ocre sends
/// the [`Delivery`] with `fetch` and logs a failed send; it never retries.
///
/// # Examples
///
/// ```
/// use ocre::errors::{Delivery, Report, Subscriber};
///
/// /// Posts each error to a webhook (a chat channel, an incident tool...).
/// struct Webhook;
///
/// impl Subscriber for Webhook {
///     fn name(&self) -> &'static str {
///         "webhook"
///     }
///
///     fn deliver(&self, report: &Report, vars: &dyn Fn(&str) -> Option<String>) -> Option<Delivery> {
///         let url = vars("ERRORS_WEBHOOK_URL")?;
///         let body = serde_json::json!({ "text": format!("{}: {}", report.source, report.message) }).to_string();
///         Some(Delivery { url, headers: vec![("content-type".into(), "application/json".into())], body })
///     }
/// }
///
/// ocre::errors::subscribe(Webhook);
/// ```
pub trait Subscriber: Send + Sync {
    /// A short name, for [`Options::except`] and failure logs: `sentry`.
    fn name(&self) -> &'static str;

    /// The request to send for `report`, if any.
    fn deliver(&self, report: &Report, vars: &dyn Fn(&str) -> Option<String>) -> Option<Delivery>;
}

static SUBSCRIBERS: Mutex<Vec<Arc<dyn Subscriber>>> = Mutex::new(Vec::new());

/// Registers a subscriber for every report of this Worker instance (Rails' `Rails.error.subscribe`).
///
/// Call it from the `start` event (see the [module documentation](self)),
/// which runs once per instance; a second subscriber with the same
/// [`name`](Subscriber::name) replaces the first.
///
/// # Examples
///
/// ```
/// ocre::errors::subscribe(ocre::errors::Sentry);
/// ```
pub fn subscribe(subscriber: impl Subscriber + 'static) {
    let mut subscribers = SUBSCRIBERS.lock().unwrap_or_else(PoisonError::into_inner);
    subscribers.retain(|existing| existing.name() != subscriber.name());
    subscribers.push(Arc::new(subscriber));
}

/// The deliveries of `reports` for every registered subscriber, with the
/// subscriber's name: what the runtime sends after a request or job.
pub(crate) fn deliveries(reports: &[Report], vars: &dyn Fn(&str) -> Option<String>) -> Vec<(&'static str, Delivery)> {
    let subscribers = SUBSCRIBERS.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let mut out = Vec::new();
    for report in reports {
        for subscriber in subscribers.iter().filter(|s| !report.except.contains(&s.name())) {
            if let Some(delivery) = subscriber.deliver(report, vars) {
                out.push((subscriber.name(), delivery));
            }
        }
    }
    out
}

#[derive(Debug, Default)]
struct State {
    context: Map<String, Value>,
    pending: Vec<Report>,
}

/// The error reporter of one request, job batch or cron run: [`Ctx::errors`](crate::Ctx::errors).
///
/// Cheap to clone; clones share their context and pending reports.
/// `Reporter::default()` builds one for unit tests, whose reports are read
/// back with [`take`](Self::take).
///
/// # Examples
///
/// ```
/// use ocre::errors::{Reporter, Severity};
///
/// let reporter = Reporter::default();
/// let value: Option<i32> = reporter.handle("x".parse::<i32>());
/// assert_eq!(value, None);
/// let reports = reporter.take();
/// assert_eq!((reports[0].handled, reports[0].severity), (true, Severity::Warning));
/// ```
#[derive(Debug, Clone, Default)]
pub struct Reporter {
    state: Arc<Mutex<State>>,
    log: Logger,
}

impl Reporter {
    /// A reporter whose log lines carry the fields of `log` (the request id...).
    pub(crate) fn new(log: Logger) -> Self {
        Self { state: Arc::default(), log }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds context to every later report of this request or job (Rails' `Rails.error.set_context`).
    ///
    /// # Examples
    ///
    /// ```
    /// let reporter = ocre::errors::Reporter::default();
    /// reporter.set_context("user_id", 7);
    /// reporter.report(&"oops", ocre::errors::Options::new());
    /// assert_eq!(reporter.take()[0].context["user_id"], 7);
    /// ```
    pub fn set_context(&self, key: &str, value: impl Serialize) {
        self.lock().context.insert(key.to_owned(), serde_json::to_value(value).unwrap_or(Value::Null));
    }

    /// Reports an error (Rails' `Rails.error.report`): logs it and queues it for the subscribers.
    ///
    /// # Examples
    ///
    /// ```
    /// let reporter = ocre::errors::Reporter::default();
    /// let err = ocre::Error::internal("webhook signature mismatch");
    /// reporter.report(&err, ocre::errors::Options::new().source("webhooks"));
    /// assert_eq!(reporter.take()[0].message, "internal error: webhook signature mismatch");
    /// ```
    pub fn report<E: Display + ?Sized>(&self, error: &E, options: Options) {
        self.push(std::any::type_name::<E>(), error.to_string(), options, true);
    }

    /// Reports the error of `result`, if any, as handled, and returns its value (Rails' `Rails.error.handle`).
    ///
    /// `.unwrap_or(fallback)` gives Rails' `fallback:`. Only errors of the
    /// result's type are caught: other errors propagate with `?` before
    /// reaching it (Rails' class filter).
    ///
    /// # Examples
    ///
    /// ```
    /// let reporter = ocre::errors::Reporter::default();
    /// assert_eq!(reporter.handle("4".parse::<u8>()), Some(4));
    /// assert_eq!(reporter.handle("x".parse::<u8>()).unwrap_or(1), 1);
    /// assert_eq!(reporter.take().len(), 1);
    /// ```
    pub fn handle<T, E: Display>(&self, result: Result<T, E>) -> Option<T> {
        result.map_err(|err| self.report(&err, Options::new())).ok()
    }

    /// Reports the error of `result`, if any, as unhandled, and returns `result` unchanged (Rails' `Rails.error.record`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::errors::{Reporter, Severity};
    ///
    /// let reporter = Reporter::default();
    /// assert!(reporter.record("x".parse::<u8>()).is_err());
    /// let report = &reporter.take()[0];
    /// assert_eq!((report.handled, report.severity), (false, Severity::Error));
    /// ```
    pub fn record<T, E: Display>(&self, result: Result<T, E>) -> Result<T, E> {
        result.inspect_err(|err| self.report(err, Options::new().handled(false).severity(Severity::Error)))
    }

    /// Reports something that should never happen (Rails' `Rails.error.unexpected`).
    ///
    /// Panics in debug builds (`ocre dev`, tests), so the bug is seen at
    /// once; in release builds it is reported as an error and the code goes on.
    ///
    /// # Panics
    ///
    /// In debug builds, always.
    ///
    /// # Examples
    ///
    /// ```should_panic
    /// ocre::errors::Reporter::default().unexpected("a paid order has no invoice");
    /// ```
    #[track_caller]
    pub fn unexpected(&self, message: impl Display) {
        self.unexpected_in(&message, cfg!(debug_assertions));
    }

    #[track_caller]
    fn unexpected_in(&self, message: &dyn Display, raise: bool) {
        assert!(!raise, "unexpected: {message}");
        let location = std::panic::Location::caller().to_string();
        let options = Options::new().severity(Severity::Error).context("location", location).source("unexpected");
        self.push("unexpected", message.to_string(), options, true);
    }

    /// Ocre's own report of an unhandled error from `source` (`ocre.request`...);
    /// `log: false` when the caller already logged its own line (jobs, cron).
    pub(crate) fn report_unhandled(&self, message: &str, source: &str, context: Map<String, Value>, log: bool) {
        let mut options = Options::new().handled(false).severity(Severity::Error).source(source);
        options.context = context;
        self.push("ocre::Error", message.to_owned(), options, log);
    }

    /// Takes the reports made so far, for the subscribers (or a test).
    ///
    /// # Examples
    ///
    /// ```
    /// let reporter = ocre::errors::Reporter::default();
    /// assert!(reporter.take().is_empty());
    /// ```
    pub fn take(&self) -> Vec<Report> {
        std::mem::take(&mut self.lock().pending)
    }

    fn push(&self, class: &str, message: String, options: Options, logged: bool) {
        let mut state = self.lock();
        let mut context = state.context.clone();
        context.extend(options.context);
        let report = Report {
            class: class.to_owned(),
            message,
            handled: options.handled,
            severity: options.severity,
            context,
            source: options.source,
            timestamp: crate::now(),
            environment: Environment::current(),
            except: options.except,
        };
        if logged {
            let prefix = if report.source.starts_with("ocre.") { "[ocre] " } else { "" };
            let mut log = self.log.with("error_class", &report.class).with("handled", report.handled);
            log = log.with("source", &report.source);
            for (key, value) in &report.context {
                log = log.with(key, value);
            }
            log.log(report.severity.level(), &format_args!("{prefix}{}", report.message));
        }
        state.pending.push(report);
    }
}

/// Worker secret holding the Sentry DSN: `https://<key>@<host>/<project id>`.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::errors::SENTRY_DSN, "SENTRY_DSN");
/// ```
pub const SENTRY_DSN: &str = "SENTRY_DSN";

/// Sends reports to Sentry or a Sentry-compatible service, as envelopes (the Sentry ingestion protocol).
///
/// Reads the DSN from the [`SENTRY_DSN`] secret (`ocre secrets push
/// SENTRY_DSN --file .prod.vars`); without it, sends nothing. The optional
/// `SENTRY_RELEASE` variable sets the release. Each report becomes one
/// event: the error as the exception (type [`Report::class`], value
/// [`Report::message`], mechanism [`Report::source`] and `handled`), the
/// severity as level, the environment, `request_id` as a tag and the
/// context (filtered like logs) as extra data. One subrequest per report.
///
/// # Examples
///
/// ```
/// use ocre::errors::{Options, Reporter, Sentry, Subscriber};
///
/// let reporter = Reporter::default();
/// reporter.report(&"boom", Options::new());
/// let report = &reporter.take()[0];
/// let dsn = |name: &str| (name == "SENTRY_DSN").then(|| "https://abc@o1.ingest.sentry.io/42".to_owned());
/// let delivery = Sentry.deliver(report, &dsn).unwrap();
/// assert_eq!(delivery.url, "https://o1.ingest.sentry.io/api/42/envelope/");
/// assert!(Sentry.deliver(report, &|_| None).is_none());
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Sentry;

impl Subscriber for Sentry {
    fn name(&self) -> &'static str {
        "sentry"
    }

    fn deliver(&self, report: &Report, vars: &dyn Fn(&str) -> Option<String>) -> Option<Delivery> {
        let dsn = vars(SENTRY_DSN)?;
        let Some((scheme, key, host, project)) = parse_dsn(&dsn) else {
            crate::error::log_internal(&format!(
                "{SENTRY_DSN} is not a Sentry DSN (https://<key>@<host>/<project id>)"
            ));
            return None;
        };
        let event_id: String = crate::token::random_bytes::<16>().iter().map(|b| format!("{b:02x}")).collect();
        let mut tags = json!({ "source": report.source });
        if let Some(id) = report.context.get("request_id") {
            tags["request_id"] = id.clone();
        }
        let mut event = json!({
            "event_id": event_id,
            "timestamp": report.timestamp,
            "platform": "other",
            "level": report.severity.as_str(),
            "logger": report.source,
            "environment": report.environment.as_str(),
            "exception": { "values": [{
                "type": report.class,
                "value": report.message,
                "mechanism": { "type": report.source, "handled": report.handled },
            }] },
            "tags": tags,
            "extra": crate::security::filter_json(&Value::Object(report.context.clone())),
            "sdk": { "name": "ocre", "version": env!("CARGO_PKG_VERSION") },
        });
        if let Some(release) = vars("SENTRY_RELEASE") {
            event["release"] = release.into();
        }
        let header = json!({ "event_id": event_id, "dsn": dsn });
        let body = format!("{header}\n{}\n{event}\n", json!({ "type": "event" }));
        let auth =
            format!("Sentry sentry_version=7, sentry_key={key}, sentry_client=ocre/{}", env!("CARGO_PKG_VERSION"));
        Some(Delivery {
            url: format!("{scheme}://{host}/api/{project}/envelope/"),
            headers: vec![
                ("content-type".to_owned(), "application/x-sentry-envelope".to_owned()),
                ("x-sentry-auth".to_owned(), auth),
            ],
            body,
        })
    }
}

/// `https://key@host/path/42` into (`https`, `key`, `host/path`, `42`).
fn parse_dsn(dsn: &str) -> Option<(&str, &str, &str, &str)> {
    let (scheme, rest) = dsn.trim().split_once("://")?;
    let (key, location) = rest.split_once('@')?;
    let key = key.split(':').next().unwrap_or(key);
    let (host, project) = location.trim_end_matches('/').rsplit_once('/')?;
    let valid =
        !key.is_empty() && !host.is_empty() && !project.is_empty() && project.bytes().all(|b| b.is_ascii_digit());
    valid.then_some((scheme, key, host, project))
}

/// What the development error page shows of the request.
#[derive(Debug, Clone, Default)]
pub(crate) struct RequestDetails {
    pub request_id: String,
    pub method: String,
    pub path: String,
    /// Filtered with `security::filter_parameters`.
    pub query: String,
    /// Filtered: `cookie`, `authorization` and sensitive names hidden.
    pub headers: Vec<(String, String)>,
}

impl RequestDetails {
    /// The request's id, method and path; the query and headers only for
    /// the development error page (`dev`).
    pub fn new(
        request_id: &str,
        method: &str,
        uri: &axum::http::Uri,
        headers: &axum::http::HeaderMap,
        dev: bool,
    ) -> Self {
        let mut details = Self {
            request_id: request_id.to_owned(),
            method: method.to_owned(),
            path: uri.path().to_owned(),
            ..Self::default()
        };
        if dev {
            details.query = crate::security::filter_parameters(uri.query().unwrap_or_default());
            details.headers = headers
                .iter()
                .map(|(name, value)| {
                    (name.to_string(), shown_header(name.as_str(), value.to_str().unwrap_or("(binary)")))
                })
                .collect();
        }
        details
    }

    /// The logger of the request: every line carries its id, method and path.
    pub fn logger(&self) -> Logger {
        Logger::new().with("request_id", &self.request_id).with("method", &self.method).with("path", &self.path)
    }
}

/// What `serve` does with the router's response: reports an
/// [`Error::Internal`](crate::Error::Internal) (logged with the request's
/// fields), sets `X-Request-Id`, logs the `debug` request line, and in
/// debug builds (`dev`) adds `Server-Timing` and shows the development
/// error page (HTML) or the internal message as `error.detail` (JSON).
pub(crate) async fn finish(
    mut response: axum::response::Response,
    request: &RequestDetails,
    reporter: &Reporter,
    timings: &crate::instrument::Timings,
    total_ms: f64,
    dev: bool,
) -> axum::response::Response {
    use axum::http::HeaderValue;

    if let Some(crate::error::InternalError(message)) = response.extensions_mut().remove() {
        let context = Map::from_iter([
            ("request_id".to_owned(), Value::from(request.request_id.as_str())),
            ("method".to_owned(), Value::from(request.method.as_str())),
            ("path".to_owned(), Value::from(request.path.as_str())),
        ]);
        reporter.report_unhandled(&message, "ocre.request", context, true);
        if dev {
            response = dev_response(response, &message, request, timings).await;
        }
    }
    if let Ok(id) = HeaderValue::from_str(&request.request_id) {
        response.headers_mut().entry("x-request-id").or_insert(id);
    }
    if dev && let Ok(value) = HeaderValue::from_str(&timings.server_timing(total_ms)) {
        response.headers_mut().insert("server-timing", value);
    }
    let status = response.status().as_u16();
    reporter.log.debug(timings.summary(&request.method, &request.path, status, total_ms));
    response
}

/// The development form of a 500: the error page with the internal
/// message, or the JSON error with `detail`.
async fn dev_response(
    response: axum::response::Response,
    message: &str,
    request: &RequestDetails,
    timings: &crate::instrument::Timings,
) -> axum::response::Response {
    use axum::http::{HeaderValue, header};

    let content_type = response.headers().get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok());
    let (html, json) =
        content_type.map_or((false, false), |value| (value.starts_with("text/html"), value.contains("json")));
    let (mut parts, body) = response.into_parts();
    let body = if html {
        dev_page(parts.status.as_u16(), message, request, &timings.statements())
    } else if json {
        let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap_or_default();
        let mut value: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({ "error": {} }));
        value["error"]["detail"] = Value::from(message);
        value.to_string()
    } else {
        return axum::response::Response::from_parts(parts, body);
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    if html {
        parts.headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    }
    axum::response::Response::from_parts(parts, axum::body::Body::from(body))
}

/// The development error page (debug builds only): the internal message,
/// the request and the D1 statements it ran (Rails' development exception page).
pub(crate) fn dev_page(
    status: u16,
    message: &str,
    request: &RequestDetails,
    statements: &[crate::instrument::Timing],
) -> String {
    let e = escape;
    let mut page = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{status} {title}</title><style>body{{font-family:system-ui,sans-serif;margin:2rem;max-width:70rem}}h1{{color:#b3261e}}pre{{background:#f6f6f6;padding:1rem;overflow:auto;white-space:pre-wrap}}table{{border-collapse:collapse}}td,th{{border:1px solid #ddd;padding:.25rem .5rem;text-align:left;vertical-align:top}}</style></head><body>",
        title = e(message.lines().next().unwrap_or(message))
    );
    page.push_str(&format!("<h1>{status} Internal server error</h1><pre>{}</pre>", e(message)));
    page.push_str("<p>This page shows because the Worker is a debug build (<code>ocre dev</code>). After <code>ocre deploy</code>, users see the error page and the details go to the logs and error reporters.</p>");
    page.push_str(&format!(
        "<h2>Request</h2><table><tr><th>Request id</th><td>{}</td></tr><tr><th>Method</th><td>{}</td></tr><tr><th>Path</th><td>{}</td></tr><tr><th>Query</th><td>{}</td></tr></table>",
        e(&request.request_id),
        e(&request.method),
        e(&request.path),
        e(&request.query)
    ));
    page.push_str("<h2>Headers</h2><table>");
    for (name, value) in &request.headers {
        page.push_str(&format!("<tr><th>{}</th><td>{}</td></tr>", e(name), e(value)));
    }
    page.push_str(&format!("</table><h2>D1 statements ({})</h2><table>", statements.len()));
    for statement in statements {
        page.push_str(&format!("<tr><td>{} ms</td><td><code>{}</code></td></tr>", statement.ms, e(&statement.sql)));
    }
    page.push_str("</table></body></html>");
    page
}

/// Header values shown on the development error page: secrets hidden.
pub(crate) fn shown_header(name: &str, value: &str) -> String {
    let hidden = ["cookie", "authorization", "proxy-authorization"].contains(&name)
        || crate::security::filter_json(&json!({ name: value }))[name] != json!(value);
    if hidden { "[FILTERED]".to_owned() } else { value.to_owned() }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
#[path = "../tests/errors.rs"]
mod tests;
