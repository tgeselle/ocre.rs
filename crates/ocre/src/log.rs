//! Structured logging to Workers Logs: levels, request-scoped fields, JSON lines (Rails' `Rails.logger` and tagged logging).
//!
//! Every [`Ctx`](crate::Ctx) carries a [`Logger`]: in a request it is tagged
//! with the request's `request_id`, `method` and `path`, so each line of a
//! request can be found together in the Workers Logs dashboard. Add fields
//! with [`Logger::with`] (Rails' `logger.tagged`):
//!
//! ```no_run
//! use axum::extract::{Path, State};
//! use ocre::{Ctx, Result};
//!
//! async fn pay(State(ctx): State<Ctx>, Path(order_id): Path<i64>) -> Result<&'static str> {
//!     let log = ctx.log().with("order_id", order_id);
//!     log.info("payment started");
//!     log.debug(format_args!("cart: {:?}", vec![1, 2])); // formatted only when debug is on
//!     Ok("OK")
//! }
//! # let _ = pay;
//! ```
//!
//! # Levels and formats
//!
//! Two Worker variables configure logging; they are read on every request,
//! job batch and cron run (no binding call):
//!
//! | Variable | Values | Default in `ocre dev` (debug build) | Default after `ocre deploy` (release build) |
//! |---|---|---|---|
//! | [`LOG_LEVEL`] | `debug`, `info`, `warn`, `error`, `off` (`warning`, `fatal` accepted) | `debug` | `info` |
//! | [`LOG_FORMAT`] | `json`, `text` | `text` | `json` |
//!
//! `json` writes each line as a JavaScript object (`{"level": "info",
//! "message": "...", "request_id": "...", ...}`): Workers Logs indexes every
//! field, so the dashboard can filter on `$metadata.request_id` or your own
//! fields. `text` writes `INFO payment started order_id=42 request_id=...`,
//! easier to read in the `ocre dev` terminal. Lines go to `console.debug`,
//! `console.info`, `console.warn` or `console.error`, so the level also shows
//! in the dashboard and in `ocre logs`.
//!
//! Field names matching [`FILTERED_PARAMETERS`](crate::security::FILTERED_PARAMETERS)
//! (`password`, `email`, `token`...) are written as `[FILTERED]`, at any depth.
//!
//! # Free plan
//!
//! Workers Logs keeps 200,000 events a day for 3 days on the free plan
//! (September 2026); each request is one event plus one per line logged.
//! Above that, events are sampled, never billed. `info` in production logs
//! only what the app asks for; Ocre's own per-request and SQL lines are
//! `debug`, so they cost nothing after `ocre deploy`. Messages passed as
//! `format_args!(...)` are only formatted when their level is on.

use std::{
    fmt::{Display, Write as _},
    sync::atomic::{AtomicU8, Ordering},
};

use serde::Serialize;
use serde_json::{Map, Value};

/// Worker variable setting the lowest level logged: `debug`, `info`, `warn`, `error` or `off`.
///
/// Set it in cloudflare.config.ts (`LOG_LEVEL: bindings.text("debug"),`)
/// or in `.dev.vars`. Unset: `debug` in debug builds (`ocre dev`), `info` in
/// release builds (`ocre deploy`).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::log::LOG_LEVEL, "LOG_LEVEL");
/// ```
pub const LOG_LEVEL: &str = "LOG_LEVEL";

/// Worker variable choosing the line format: `json` or `text`.
///
/// Unset: `text` in debug builds (`ocre dev`), `json` in release builds.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::log::LOG_FORMAT, "LOG_FORMAT");
/// ```
pub const LOG_FORMAT: &str = "LOG_FORMAT";

/// Severity of a log line (Rails' `:debug`, `:info`, `:warn`, `:error`).
///
/// Rails' `:fatal` and `:unknown` are [`Error`](Self::Error) here.
///
/// # Examples
///
/// ```
/// use ocre::log::Level;
///
/// assert!(Level::Debug < Level::Error);
/// assert_eq!(Level::parse("WARNING"), Some(Level::Warn));
/// assert_eq!(Level::Info.as_str(), "info");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Details for development: SQL statements, request summaries.
    Debug,
    /// Normal events worth keeping in production.
    Info,
    /// Something unexpected that the app handled.
    Warn,
    /// A failure: internal errors, failed jobs.
    Error,
}

impl Level {
    /// Reads a level name, case-insensitively: `debug`, `info`, `warn` (`warning`), `error` (`fatal`, `unknown`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::log::Level;
    ///
    /// assert_eq!(Level::parse("fatal"), Some(Level::Error));
    /// assert_eq!(Level::parse("loud"), None);
    /// ```
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warn),
            "error" | "fatal" | "unknown" => Some(Self::Error),
            _ => None,
        }
    }

    /// The lowercase name written in log lines: `debug`, `info`, `warn`, `error`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::log::Level::Warn.as_str(), "warn");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// How log lines are written: see the [module documentation](self).
///
/// # Examples
///
/// ```
/// use ocre::log::Format;
///
/// assert_eq!(Format::parse("JSON"), Some(Format::Json));
/// assert_eq!(Format::parse("pretty"), Some(Format::Text));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One JavaScript object per line, indexed by Workers Logs.
    Json,
    /// `LEVEL message key=value ...`, for the terminal.
    Text,
}

impl Format {
    /// Reads a format name: `json`, or `text` (`pretty` and `compact` accepted, like Loco).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::log::Format::parse("yaml"), None);
    /// ```
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "text" | "pretty" | "compact" => Some(Self::Text),
            _ => None,
        }
    }
}

/// Current settings: 0 = not configured yet; else `1 + threshold * 2 + json`,
/// where threshold 4 means `off`.
static SETTINGS: AtomicU8 = AtomicU8::new(0);
const OFF: u8 = 4;

/// Applies the [`LOG_LEVEL`] and [`LOG_FORMAT`] values of the Worker
/// (`None` or an unknown value: the build's default). Called by `Ctx::new`.
pub(crate) fn configure(level: Option<&str>, format: Option<&str>) {
    let threshold = match level.map(|name| name.trim().to_ascii_lowercase()) {
        Some(name) if name == "off" || name == "none" => OFF,
        Some(name) => Level::parse(&name).unwrap_or_else(default_level) as u8,
        None => default_level() as u8,
    };
    let json = format.and_then(Format::parse).unwrap_or_else(default_format) == Format::Json;
    SETTINGS.store(1 + threshold * 2 + u8::from(json), Ordering::Relaxed);
}

fn default_level() -> Level {
    if cfg!(debug_assertions) { Level::Debug } else { Level::Info }
}

fn default_format() -> Format {
    if cfg!(debug_assertions) { Format::Text } else { Format::Json }
}

fn settings() -> (u8, Format) {
    match SETTINGS.load(Ordering::Relaxed) {
        0 => (default_level() as u8, default_format()),
        packed => ((packed - 1) / 2, if (packed - 1) % 2 == 1 { Format::Json } else { Format::Text }),
    }
}

/// Writes log lines with a set of fields; cheap to clone (see the [module documentation](self)).
///
/// [`Ctx::log`](crate::Ctx::log) gives the request's logger; code without a
/// `Ctx` uses [`Logger::new`].
///
/// # Examples
///
/// ```
/// use ocre::log::{Format, Level, Logger};
///
/// let log = Logger::new().with("order_id", 42).with("password", "hunter2");
/// log.info("paid"); // written to the Worker logs
/// assert_eq!(
///     log.line(Level::Info, "paid", Format::Json),
///     r#"{"level":"info","message":"paid","order_id":42,"password":"[FILTERED]"}"#
/// );
/// assert_eq!(log.line(Level::Warn, "slow", Format::Text), "WARN slow order_id=42 password=[FILTERED]");
/// ```
#[derive(Debug, Clone, Default)]
pub struct Logger {
    fields: Map<String, Value>,
}

impl Logger {
    /// A logger without fields.
    ///
    /// # Examples
    ///
    /// ```
    /// ocre::log::Logger::new().warn("cache miss");
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// A copy of this logger with one more field on every line (Rails' `logger.tagged`).
    ///
    /// Any serializable value; a value that does not serialize is written
    /// as `null`. A field with the same name is replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// let log = ocre::log::Logger::new().with("user_id", 7);
    /// assert_eq!(log.field("user_id"), Some(&serde_json::json!(7)));
    /// ```
    pub fn with(&self, key: &str, value: impl Serialize) -> Self {
        let mut logger = self.clone();
        logger.fields.insert(key.to_owned(), serde_json::to_value(value).unwrap_or(Value::Null));
        logger
    }

    /// The value of a field, e.g. `request_id`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::log::Logger::new().field("request_id"), None);
    /// ```
    pub fn field(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    /// Whether lines of `level` are written, under the current [`LOG_LEVEL`].
    ///
    /// Check it before building an expensive message that is not
    /// `format_args!(...)` (Rails' block form of `logger.debug { ... }`).
    ///
    /// # Examples
    ///
    /// ```
    /// let log = ocre::log::Logger::new();
    /// if log.enabled(ocre::log::Level::Debug) {
    ///     log.debug(serde_json::json!({"big": [1, 2, 3]}));
    /// }
    /// ```
    pub fn enabled(&self, level: Level) -> bool {
        level as u8 >= settings().0
    }

    /// Writes a `debug` line.
    ///
    /// # Examples
    ///
    /// ```
    /// let rows = 3;
    /// ocre::log::Logger::new().debug(format_args!("{rows} rows"));
    /// ```
    pub fn debug(&self, message: impl Display) {
        self.log(Level::Debug, &message);
    }

    /// Writes an `info` line.
    ///
    /// # Examples
    ///
    /// ```
    /// ocre::log::Logger::new().info("signed up");
    /// ```
    pub fn info(&self, message: impl Display) {
        self.log(Level::Info, &message);
    }

    /// Writes a `warn` line.
    ///
    /// # Examples
    ///
    /// ```
    /// ocre::log::Logger::new().warn("retrying");
    /// ```
    pub fn warn(&self, message: impl Display) {
        self.log(Level::Warn, &message);
    }

    /// Writes an `error` line.
    ///
    /// # Examples
    ///
    /// ```
    /// ocre::log::Logger::new().error("payment provider down");
    /// ```
    pub fn error(&self, message: impl Display) {
        self.log(Level::Error, &message);
    }

    /// Writes a line of `level` when it is enabled; the message is formatted only then.
    ///
    /// # Examples
    ///
    /// ```
    /// ocre::log::Logger::new().log(ocre::log::Level::Info, &"hello");
    /// ```
    pub fn log(&self, level: Level, message: &dyn Display) {
        if !self.enabled(level) {
            return;
        }
        let format = settings().1;
        emit(level, format, &self.line(level, &message.to_string(), format));
    }

    /// The line [`log`](Self::log) writes, whatever the current level: JSON or text.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::log::{Format, Level, Logger};
    ///
    /// let line = Logger::new().with("path", "/a b").line(Level::Error, "boom", Format::Text);
    /// assert_eq!(line, r#"ERROR boom path="/a b""#);
    /// ```
    pub fn line(&self, level: Level, message: &str, format: Format) -> String {
        let fields = crate::security::filter_json(&Value::Object(self.fields.clone()));
        let Value::Object(fields) = fields else { unreachable!("filtering keeps an object") };
        match format {
            Format::Json => {
                let mut line = Map::with_capacity(fields.len() + 2);
                line.insert("level".to_owned(), level.as_str().into());
                line.insert("message".to_owned(), message.into());
                line.extend(fields);
                Value::Object(line).to_string()
            }
            Format::Text => {
                let mut line = format!("{} {message}", level.as_str().to_ascii_uppercase());
                for (key, value) in fields {
                    let _ = write!(line, " {key}={}", text_value(&value));
                }
                line
            }
        }
    }
}

/// Strings bare unless they contain spaces, quotes or `=`; other values as JSON.
fn text_value(value: &Value) -> String {
    match value {
        Value::String(text) if !text.is_empty() && !text.contains([' ', '"', '=']) => text.clone(),
        other => other.to_string(),
    }
}

/// `panicked at src/lib.rs:12:5: <message>`: the line the panic hook logs.
pub(crate) fn panic_line(message: &str, location: Option<(&str, u32, u32)>) -> String {
    match location {
        Some((file, line, column)) => format!("panicked at {file}:{line}:{column}: {message}"),
        None => format!("panicked: {message}"),
    }
}

/// Worker logs: `console.<level>` with a JavaScript object for JSON lines;
/// stderr in native builds (unit tests).
fn emit(level: Level, format: Format, line: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use worker::wasm_bindgen::JsValue;
        let value = match format {
            Format::Json => js_sys::JSON::parse(line).unwrap_or_else(|_| JsValue::from_str(line)),
            Format::Text => JsValue::from_str(line),
        };
        let console = match level {
            Level::Debug => worker::web_sys::console::debug_1,
            Level::Info => worker::web_sys::console::info_1,
            Level::Warn => worker::web_sys::console::warn_1,
            Level::Error => worker::web_sys::console::error_1,
        };
        console(&value);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (level, format);
        eprintln!("{line}");
    }
}

#[cfg(test)]
#[path = "../tests/log.rs"]
mod tests;
