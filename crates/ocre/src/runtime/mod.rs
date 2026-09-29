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
pub(crate) mod oauth;
mod query;
#[cfg(feature = "realtime")]
pub(crate) mod realtime;
pub(crate) mod security;
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

/// Runs one request through the application router: the Worker `fetch` entry point.
///
/// Builds a [`Ctx`] from `env`, gives it to `routes` as axum state, and wraps
/// the router with the middleware every Ocre app runs (outermost first):
///
/// 1. Security headers on every response (`X-Content-Type-Options: nosniff`,
///    `X-Frame-Options: SAMEORIGIN`, `Referrer-Policy`, HSTS on HTTPS...); a
///    handler's own value wins.
/// 2. Host authorization: when the [`ALLOWED_HOSTS`](crate::ALLOWED_HOSTS)
///    Worker variable is set, other hosts get `403 Forbidden`.
/// 3. CORS for the origins listed in the [`ALLOWED_ORIGINS`](crate::ALLOWED_ORIGINS)
///    Worker variable (no CORS layer when it is empty).
/// 4. Cross-origin request protection (CSRF) without tokens: unsafe requests a
///    browser sends from another site (`Sec-Fetch-Site`, or `Origin` against
///    `Host`) get `403 Forbidden`.
/// 5. The encrypted cookie [`Session`](crate::Session), keyed from the
///    [`SECRET_KEY_BASE`](crate::SECRET_KEY_BASE) secret (and, during a
///    rotation, [`SECRET_KEY_BASE_PREVIOUS`](crate::SECRET_KEY_BASE_PREVIOUS)).
///
/// A missing or short `SECRET_KEY_BASE` does not fail every request: only
/// handlers that touch the session get [`Error::Internal`](crate::Error::Internal),
/// naming the fix (`ocre secret`, `.dev.vars`).
///
/// The request also carries the [`Ctx`] as an extension, so the app's own
/// middleware (`axum::middleware::from_fn`) can reach the bindings with an
/// `Extension(ctx): Extension<Ctx>` argument, as handlers do with `State`.
///
/// Files from [`storage::serve`](crate::storage::serve) go out as R2's own
/// stream, so the Worker spends no CPU copying them and `Content-Length` is
/// kept.
///
/// # Errors
///
/// Returns a [`worker::Error`] only when the response cannot be converted to a
/// JavaScript `Response`. Handler errors are responses (an HTML page or JSON),
/// not `Err`.
///
/// # Free plan
///
/// One call per Worker request (100,000 a day); the middleware itself reads no
/// D1 rows and no KV keys: sessions live in the cookie.
///
/// # Examples
///
/// `src/lib.rs` of a generated app:
///
/// ```no_run
/// use axum::{Router, routing::get};
/// use ocre::Ctx;
///
/// fn routes() -> Router<Ctx> {
///     Router::new().route("/up", get(|| async { "OK" }))
/// }
///
/// #[worker::event(fetch)]
/// async fn fetch(
///     req: worker::HttpRequest,
///     env: worker::Env,
///     _ctx: worker::Context,
/// ) -> worker::Result<worker::web_sys::Response> {
///     ocre::serve(routes(), req, env).await
/// }
/// # fn main() {}
/// ```
pub async fn serve(routes: Router<Ctx>, req: HttpRequest, env: Env) -> worker::Result<web_sys::Response> {
    let var = |name: &str| env.var(name).ok().map(|var| var.to_string());
    let secret = |name: &str| env.secret(name).ok().map(|secret| secret.to_string());
    let config = protect::Config {
        keys: session::keys_from_secrets(secret(session::SECRET_KEY_BASE), secret(session::SECRET_KEY_BASE_PREVIOUS)),
        allowed_origins: protect::parse_origins(var(protect::ALLOWED_ORIGINS)),
        allowed_hosts: protect::parse_hosts(var(protect::ALLOWED_HOSTS)),
    };
    let ctx = Ctx::new(env);
    let mut req = req;
    req.extensions_mut().insert(ctx.clone());
    let mut app = protect::wrap(routes.with_state(ctx), config);
    let mut response = app.call(req).await?;
    match response.extensions_mut().remove::<storage::R2Stream>() {
        Some(stream) => storage::into_js_response(response, stream),
        None => worker::response_to_wasm(response),
    }
}
