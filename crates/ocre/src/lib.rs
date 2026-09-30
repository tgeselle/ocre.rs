//! Ocre: a Rails-like Rust web framework for Cloudflare Workers, built to run
//! on the Workers free plan and to be written by AI agents.
//!
//! An Ocre app is one Worker compiled to WebAssembly. Requests go through
//! [`serve`], which runs a plain [axum](https://docs.rs/axum) router with
//! sessions, CSRF protection, CORS and security headers. Handlers reach the
//! Worker's bindings (D1, KV, R2, Queues, Durable Objects, email) through the
//! per-request [`Ctx`]. The `ocre` command-line tool (crate `ocre-cli`)
//! generates the app, its models, scaffolds and migrations, and deploys it.
//!
//! Guides and the generated app's conventions live in the repository
//! [README](https://github.com/tgeselle/ocre.rs#readme); a one-page list of
//! every public item is in `docs/api-index.md`.
//!
//! # Design rules
//!
//! - **Plain axum handlers.** Every Ocre type is `Send`, so handlers never
//!   need `#[worker::send]`. Extractors ([`Session`], [`Flash`], [`Json`],
//!   [`Page`], [`storage::Multipart`], [`i18n::I18n`], [`cache::Conditional`])
//!   and responses ([`Created`], [`cache::CacheControl`], [`cache::ETag`])
//!   are ordinary axum types.
//! - **One way to do each thing.** SQL with `?N` placeholders and
//!   [`params!`], askama templates compiled at build time, htmx for
//!   interactivity, a single [`Error`] type that knows its HTTP status.
//! - **Errors name the fix.** A missing binding answers 500 and logs which
//!   `cloudflare.config.ts` entry to add; internal details are logged, never shown
//!   to users.
//! - **Free plan first.** Nothing costs a request, a KV write or a database
//!   row unless the app asks for it: sessions live in an encrypted cookie
//!   (no storage), static files are served by Workers Static Assets before
//!   the Worker runs, R2 downloads stream without passing through
//!   WebAssembly, and features that add binary size or startup CPU
//!   (GraphQL, realtime) are opt-in cargo features. The free plan allows
//!   10 ms of CPU per request; functions note their cost in D1 rows, KV
//!   operations, Queue operations, R2 operations or CPU where it matters.
//!
//! # Modules
//!
//! | Module | Contents |
//! |---|---|
//! | crate root | [`serve`], [`Ctx`], [`Db`], [`params!`] and [`Query`] / [`Paginated`] (D1), [`Error`] / [`Result`], [`Json`] / [`ApiError`] / [`Page`] (JSON APIs), [`Session`] / [`Flash`] / [`Cookies`], [`Validator`], [`NestedForm`] (bracketed form names), request helpers ([`Format`], [`RemoteIp`], [`RequestId`], [`redirect_back`]), `render` / `error_page` / `Htmx` / `HxRedirect` (feature `html`), serde helpers ([`optional`], [`patch`], [`bool_from_sql`], ...) |
//! | [`bulk`] | Many rows in one D1 statement: `bulk::insert`, `upsert` and `update` (one JSON parameter, `json_each`) |
//! | [`cache`] | Read-through values in Workers KV, `Cache-Control`, `ETag` and `304 Not Modified` |
//! | [`config`] | Typed app settings from Worker variables and secrets (`ctx.config::<Settings>()`), the environment (development or production) |
//! | [`encryption`] | Encrypted model columns (AES-256-GCM keyed from `SECRET_KEY_BASE`), deterministic for lookups |
//! | [`errors`] | Error reporting (Rails' `Rails.error`): `ctx.errors().report / handle / record`, subscribers such as Sentry |
//! | [`events`] | Structured events (Rails' `Rails.event`): `ctx.events().notify(name, payload)`, tags, context, subscribers |
#![cfg_attr(
    feature = "html",
    doc = "| [`filters`] | Ocre's view helpers as askama filters: `{{ price\\|number_to_currency(\"$\") }}` (feature `html`) |"
)]
#![cfg_attr(
    not(feature = "html"),
    doc = "| `filters` | View helpers as askama filters (feature `html`, off in this build) |"
)]
#![cfg_attr(
    feature = "graphql",
    doc = "| [`graphql`] | `/graphql` endpoint and GraphiQL for an async-graphql schema (feature `graphql`) |"
)]
#![cfg_attr(
    not(feature = "graphql"),
    doc = "| `graphql` | `/graphql` endpoint and GraphiQL (feature `graphql`, off in this build) |"
)]
//! | [`helpers`] | Rails' view helpers: numbers (`number_to_currency`...), times (`time_ago_in_words`, `strftime`), text (`excerpt`, `highlight`) |
//! | [`i18n`] | Translations from `locales/*.yml`, plurals, the request's locale |
//! | [`jobs`] | Background jobs on Cloudflare Queues, scheduled tasks on Cron Triggers |
//! | [`jwt`] | HS256 JSON Web Tokens for API clients |
//! | [`log`] | Structured logging to Workers Logs: levels, request-scoped fields (`ctx.log()`), JSON lines |
//! | [`mail`] | Sending email (log, Resend, Cloudflare adapters) and receiving it from Email Routing |
//! | [`oauth`] | "Sign in with GitHub / Google": OAuth 2.0 code flow with PKCE |
//! | [`password`] | PBKDF2-HMAC-SHA256 password digests |
#![cfg_attr(
    feature = "realtime",
    doc = "| [`realtime`] | WebSocket channels on a Durable Object, htmx broadcasts (feature `realtime`) |"
)]
#![cfg_attr(
    not(feature = "realtime"),
    doc = "| `realtime` | WebSocket channels on a Durable Object (feature `realtime`, off in this build) |"
)]
//! | [`replicas`] | D1 read replicas: reads from a nearby copy, each visitor still reading their own writes (`D1_REPLICAS=on`) |
//! | [`security`] | Content-Security-Policy (nonces), Permissions-Policy, rate limits, safe redirects, `sanitize` / `strip_tags`, log filtering, HTTP Basic auth |
//! | [`sse`] | Server-Sent Events: stream events to the browser as they happen |
//! | [`storage`] | Files in Cloudflare R2: multipart uploads, attachments, streamed downloads |
//! | [`token`] | Random tokens for emailed links and API keys, stored as SHA-256 digests |
//!
//! # A complete app
//!
//! A generated app's `src/lib.rs` (crate type `cdylib`) is the Worker entry
//! point plus an axum router whose state is [`Ctx`]:
//!
//! ```no_run
//! use axum::{Router, extract::{Path, State}, routing::get};
//! use ocre::{ApiResult, Ctx, Json, OptionExt, params};
//! use serde::{Deserialize, Serialize};
//! use worker::{Context, Env, HttpRequest, event};
//!
//! #[event(fetch)]
//! async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> worker::Result<worker::web_sys::Response> {
//!     ocre::serve(routes(), req, env).await
//! }
//!
//! fn routes() -> Router<Ctx> {
//!     Router::new().route("/up", get(up)).route("/posts/{id}", get(show))
//! }
//!
//! async fn up() -> &'static str {
//!     "OK"
//! }
//!
//! #[derive(Serialize, Deserialize)]
//! struct Post {
//!     id: i64,
//!     title: String,
//! }
//!
//! // GET /posts/1: the row as JSON, or a JSON 404 when there is none. `ApiResult`
//! // answers errors as JSON; HTML pages return `ocre::Result` (feature `html`).
//! async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> ApiResult<Json<Post>> {
//!     let post = ctx.db()?.first::<Post>("SELECT id, title FROM posts WHERE id = ?1", params![id]).await?;
//!     Ok(Json(post.or_404()?))
//! }
//! # fn main() {}
//! ```
//!
//! `cloudflare.config.ts` binds the D1 database as `DB`; `ocre new` writes it, and
//! `ocre dev` / `ocre deploy` run it.
//!
//! # Cargo features
//!
//! | Feature | Default | Enables |
//! |---|---|---|
//! | `html` | yes | askama templates (`render`), HTML error pages, the `Htmx` extractor. API-only apps (`ocre new --api`) turn it off |
//! | `graphql` | no | The `graphql` module (async-graphql). About 1.1 MB more WebAssembly and 20-60 ms of CPU when a Worker instance starts |
//! | `realtime` | no | The `realtime` module and the exported `OcreChannel` Durable Object class (WebSocket Hibernation) |
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

mod api;
pub mod bulk;
pub mod cache;
mod clock;
pub mod config;
mod cookies;
pub mod encryption;
mod error;
pub mod errors;
pub mod events;
mod fields;
#[cfg(feature = "html")]
#[cfg_attr(docsrs, doc(cfg(feature = "html")))]
pub mod filters;
mod form;
#[cfg(feature = "graphql")]
#[cfg_attr(docsrs, doc(cfg(feature = "graphql")))]
pub mod graphql;
pub mod helpers;
#[cfg(feature = "html")]
mod htmx;
pub mod i18n;
mod instrument;
pub mod jobs;
pub mod jwt;
pub mod log;
pub mod mail;
mod names;
pub mod oauth;
pub mod password;
mod protect;
mod query;
#[cfg(feature = "realtime")]
#[cfg_attr(docsrs, doc(cfg(feature = "realtime")))]
pub mod realtime;
pub mod replicas;
mod request;
mod runtime;
pub mod security;
mod session;
mod sql;
pub mod sse;
pub mod storage;
#[cfg(test)]
#[path = "../tests/support.rs"]
mod support;
#[cfg(all(feature = "testing", not(target_arch = "wasm32")))]
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
pub mod testing;
pub mod token;
mod validate;
#[cfg(feature = "html")]
mod view;

pub use api::{ApiError, ApiResult, Created, Json, Page, PageLinks};
pub use clock::now;
pub use cookies::Cookies;
pub use error::{Error, OptionExt, Result};
pub use fields::{optional, patch, patch_json};
pub use form::NestedForm;
#[cfg(feature = "html")]
#[cfg_attr(docsrs, doc(cfg(feature = "html")))]
pub use htmx::{Htmx, HxRedirect};
pub use protect::{ALLOWED_HOSTS, ALLOWED_ORIGINS};
pub use query::{Batches, Direction, Paginated, Query, escape_like};
pub use request::{Format, Markdown, RemoteIp, RequestId, encode_path, redirect_back, remote_ip};
pub use runtime::{Ctx, Db, serve, sleep};
/// JSON values (`serde_json::Value`, the `json!` macro) for `json` fields,
/// without adding `serde_json` to the app.
pub use serde_json;
pub use session::{Flash, SECRET_KEY_BASE, SECRET_KEY_BASE_PREVIOUS, SESSION_COOKIE, Session};
pub use sql::{IntoParam, MAX_SAFE_INTEGER, Param, Statement, bool_from_sql, json_from_sql, optional_json_from_sql};
pub use validate::{FieldError, Validator};
#[cfg(feature = "html")]
#[cfg_attr(docsrs, doc(cfg(feature = "html")))]
pub use view::{ErrorPage, error_page, render};

/// Builds the parameter list of a [`Db`] query: `params![title, id]`.
///
/// Each value goes through [`IntoParam`], so strings, integers, floats,
/// booleans, `Option`s of those (`None` binds `NULL`) and JSON values can be
/// mixed. Values bind to the `?1`, `?2`, ... placeholders in order. The result
/// is a `Vec<Param>`, the type every [`Db`] method and [`Statement::new`]
/// take; `params![]` binds nothing.
///
/// # Examples
///
/// ```
/// use ocre::{Param, params};
///
/// let title = "Hello";
/// let published: Option<bool> = None;
/// let params: Vec<Param> = params![title, 42, published];
/// assert_eq!(params.len(), 3);
/// let none: Vec<Param> = params![];
/// assert!(none.is_empty());
/// ```
#[macro_export]
macro_rules! params {
    ($($value:expr),* $(,)?) => {
        ::std::vec![$($crate::IntoParam::into_param($value)),*]
    };
}
