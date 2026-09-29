//! Code that calls the Workers JavaScript runtime. It only runs inside
//! workerd, so it is exercised by the end-to-end tests
//! (`crates/ocre-cli/tests/system/e2e.rs`) rather than by native unit tests.

pub(crate) mod cache;
#[cfg(target_arch = "wasm32")]
mod crypto;
mod ctx;
mod d1;
#[cfg(feature = "graphql")]
mod graphql;
pub(crate) mod jobs;
pub(crate) mod jwt;
pub(crate) mod mail;
#[cfg(feature = "realtime")]
pub(crate) mod realtime;
pub(crate) mod storage;

#[cfg(target_arch = "wasm32")]
pub(crate) use crypto::{pbkdf2_sha256, unix_millis};
pub use ctx::Ctx;
pub use d1::Db;
#[cfg(feature = "graphql")]
pub use graphql::routes as graphql_routes;

use axum::Router;
use tower_service::Service;
use worker::{Env, HttpRequest, web_sys};

use crate::{protect, session};

/// Runs one request through the application router.
///
/// Call it from the Worker entry point:
///
/// ```ignore
/// #[worker::event(fetch)]
/// async fn fetch(req: worker::HttpRequest, env: worker::Env, _: worker::Context)
///     -> worker::Result<worker::web_sys::Response> {
///     ocre::serve(routes(), req, env).await
/// }
/// ```
///
/// It adds sessions, CSRF protection, CORS (`ALLOWED_ORIGINS`) and security
/// headers; see the `protect` module. Files from
/// [`storage::serve`](crate::storage::serve) go out as R2's own stream, so
/// the Worker spends no CPU copying them and `Content-Length` is kept.
pub async fn serve(routes: Router<Ctx>, req: HttpRequest, env: Env) -> worker::Result<web_sys::Response> {
    let config = protect::Config {
        key: session::key_from_secret(env.secret(session::SECRET_KEY_BASE).ok().map(|secret| secret.to_string())),
        allowed_origins: protect::parse_origins(env.var(protect::ALLOWED_ORIGINS).ok().map(|var| var.to_string())),
    };
    let mut app = protect::wrap(routes.with_state(Ctx::new(env)), config);
    let mut response = app.call(req).await?;
    match response.extensions_mut().remove::<storage::R2Stream>() {
        Some(stream) => storage::into_js_response(response, stream),
        None => worker::response_to_wasm(response),
    }
}
