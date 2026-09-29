//! Code that calls the Workers JavaScript runtime. It only runs inside
//! workerd, so it is exercised by the end-to-end tests
//! (`crates/ocre-cli/tests/e2e.rs`) rather than by native unit tests.

mod ctx;
mod d1;
#[cfg(feature = "graphql")]
mod graphql;

pub use ctx::Ctx;
pub use d1::Db;
#[cfg(feature = "graphql")]
pub use graphql::routes as graphql_routes;

use axum::{Router, body::Body, http::Response};
use tower_service::Service;
use worker::{Env, HttpRequest};

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
pub async fn serve(routes: Router<Ctx>, req: HttpRequest, env: Env) -> worker::Result<Response<Body>> {
    let mut app = routes.with_state(Ctx::new(env));
    Ok(app.call(req).await?)
}
