//! Ocre: a Rails-like Rust web framework for Cloudflare Workers, built to run
//! on the Workers free plan and to be written by AI agents.
//!
//! Design rules:
//! - Plain axum handlers. Every Ocre type is `Send`, so handlers never need
//!   `#[worker::send]`.
//! - One way to do each thing; failures surface at compile time or as errors
//!   that name the fix.

mod api;
mod error;
mod fields;
/// GraphQL support (feature `graphql`).
#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "html")]
mod htmx;
mod names;
mod runtime;
mod sql;
#[cfg(test)]
mod test_util;
mod validate;
#[cfg(feature = "html")]
mod view;

pub use api::{ApiError, ApiResult, Created, Json, Page};
pub use error::{Error, OptionExt, Result};
pub use fields::{optional, patch};
#[cfg(feature = "html")]
pub use htmx::Htmx;
pub use runtime::{Ctx, Db, serve};
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
