//! Ocre: a Rails-like Rust web framework for Cloudflare Workers, built to run
//! on the Workers free plan and to be written by AI agents.
//!
//! Design rules:
//! - Plain axum handlers. Every Ocre type is `Send`, so handlers never need
//!   `#[worker::send]`.
//! - One way to do each thing; failures surface at compile time or as errors
//!   that name the fix.

mod error;
mod htmx;
mod runtime;
mod sql;
#[cfg(test)]
mod test_util;
mod view;

pub use error::{Error, OptionExt, Result};
pub use htmx::Htmx;
pub use runtime::{Ctx, Db, serve};
pub use sql::{IntoParam, MAX_SAFE_INTEGER, Param, bool_from_sql};
pub use view::render;

/// Builds query parameters for [`Db`] methods: `params![title, id]`.
#[macro_export]
macro_rules! params {
    ($($value:expr),* $(,)?) => {
        ::std::vec![$($crate::IntoParam::into_param($value)),*]
    };
}
