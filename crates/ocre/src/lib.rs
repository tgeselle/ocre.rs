//! Ocre: a Rails-like Rust web framework for Cloudflare Workers, built to run
//! on the Workers free plan and to be written by AI agents.
//!
//! Design rules:
//! - Plain axum handlers. Every Ocre type is `Send`, so handlers never need
//!   `#[worker::send]`.
//! - One way to do each thing; failures surface at compile time or as errors
//!   that name the fix.

mod ctx;
mod db;
mod error;
mod htmx;
mod view;

pub use ctx::Ctx;
pub use db::{Db, IntoParam, Param};
pub use error::{Error, OptionExt, Result};
pub use htmx::Htmx;
pub use view::render;

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

/// Builds query parameters for [`Db`] methods: `params![title, id]`.
#[macro_export]
macro_rules! params {
    ($($value:expr),* $(,)?) => {
        ::std::vec![$($crate::IntoParam::into_param($value)),*]
    };
}
