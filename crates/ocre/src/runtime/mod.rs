//! Code that calls the Workers JavaScript runtime. It only runs inside
//! workerd, so it is exercised by the end-to-end tests
//! (`crates/ocre-cli/tests/system/e2e.rs`) rather than by native unit tests.

#[cfg(target_arch = "wasm32")]
mod crypto;
mod ctx;
mod d1;
#[cfg(feature = "graphql")]
mod graphql;
pub(crate) mod jwt;
pub(crate) mod mail;

#[cfg(target_arch = "wasm32")]
pub(crate) use crypto::{pbkdf2_sha256, unix_millis};
pub use ctx::Ctx;
pub use d1::Db;
#[cfg(feature = "graphql")]
pub use graphql::routes as graphql_routes;

use axum::{Router, body::Body, http::Response};
use tower_service::Service;
use worker::{Env, HttpRequest};

use crate::{protect, session};

/// Runs one request through the application router.
///
/// Call it from the Worker entry point:
///
/// ```ignore
/// #[worker::event(fetch)]
/// async fn fetch(req: worker::HttpRequest, env: worker::Env, _: worker::Context)
///     -> worker::Result<axum::http::Response<axum::body::Body>> {
///     ocre::serve(routes(), req, env).await
/// }
/// ```
///
/// It adds sessions, CSRF protection, CORS (`ALLOWED_ORIGINS`) and security
/// headers; see the `protect` module.
pub async fn serve(routes: Router<Ctx>, req: HttpRequest, env: Env) -> worker::Result<Response<Body>> {
    let config = protect::Config {
        key: session::key_from_secret(env.secret(session::SECRET_KEY_BASE).ok().map(|secret| secret.to_string())),
        allowed_origins: protect::parse_origins(env.var(protect::ALLOWED_ORIGINS).ok().map(|var| var.to_string())),
    };
    let mut app = protect::wrap(routes.with_state(Ctx::new(env)), config);
    Ok(app.call(req).await?)
}
