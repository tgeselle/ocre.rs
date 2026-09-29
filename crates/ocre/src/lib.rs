//! Ocre: a Rails-like Rust web framework for Cloudflare Workers, built to run
//! on the Workers free plan and to be written by AI agents.
//!
//! Design rules:
//! - Plain axum handlers. Every Ocre type is `Send`, so handlers never need
//!   `#[worker::send]`.
//! - One way to do each thing; failures surface at compile time or as errors
//!   that name the fix.

mod api;
/// Caching: read-through values in Workers KV, HTTP `Cache-Control`/`ETag` and 304 responses.
pub mod cache;
mod clock;
mod error;
mod fields;
/// GraphQL support (feature `graphql`).
#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "html")]
mod htmx;
/// Translations: `locales/*.yml`, `%{name}` interpolation, plurals, locale per request.
pub mod i18n;
/// Background jobs (Cloudflare Queues) and scheduled tasks (Cron Triggers).
pub mod jobs;
/// JSON Web Tokens (HS256) for API clients.
pub mod jwt;
/// Email: send with adapters (log, Resend, Cloudflare), receive from Email Routing.
pub mod mail;
mod names;
/// Password hashing (PBKDF2-HMAC-SHA256).
pub mod password;
mod protect;
/// Realtime updates: WebSocket channels on a Durable Object, HTML broadcasts for htmx (feature `realtime`).
#[cfg(feature = "realtime")]
pub mod realtime;
mod runtime;
mod session;
mod sql;
/// File storage in Cloudflare R2: multipart uploads, attachments, streamed downloads.
pub mod storage;
#[cfg(test)]
#[path = "../tests/support.rs"]
mod support;
/// Random tokens for emailed links and API keys, stored as digests.
pub mod token;
mod validate;
#[cfg(feature = "html")]
mod view;

pub use api::{ApiError, ApiResult, Created, Json, Page};
pub use clock::now;
pub use error::{Error, OptionExt, Result};
pub use fields::{optional, patch};
#[cfg(feature = "html")]
pub use htmx::Htmx;
pub use protect::ALLOWED_ORIGINS;
pub use runtime::{Ctx, Db, serve};
pub use session::{Flash, SECRET_KEY_BASE, SESSION_COOKIE, Session};
pub use sql::{IntoParam, MAX_SAFE_INTEGER, Param, Statement, bool_from_sql};
pub use validate::{FieldError, Validator};
#[cfg(feature = "html")]
pub use view::render;

/// Builds query parameters for [`Db`] methods: `params![title, id]`.
#[macro_export]
macro_rules! params {
    ($($value:expr),* $(,)?) => {
        ::std::vec![$($crate::IntoParam::into_param($value)),*]
    };
}
