//! D1 read replicas: reads go to a nearby copy of the database, writes to
//! the primary, and each visitor still reads what they wrote (Rails'
//! automatic role switching, without a second database to configure).
//!
//! Turn it on in two places:
//!
//! 1. enable read replication on the database (Cloudflare dashboard, D1,
//!    Settings), free on every plan;
//! 2. set the Worker variable `D1_REPLICAS` to `on`
//!    (`D1_REPLICAS: bindings.text("on")` in cloudflare.config.ts).
//!
//! Each request handled by [`serve`](crate::serve) then queries its
//! databases through one D1 session: a `GET` or `HEAD` may start on any
//! replica, other methods start on the primary, and later statements of the
//! request see the earlier ones. After a request that wrote, the response
//! sets a short-lived cookie holding the session's bookmark
//! (`ocre_d1_<binding>`, 5 minutes); the visitor's next requests resume
//! from it, so a page shown after a form never misses the new row.
//! Responses of read-only requests set no cookie and stay cacheable.
//!
//! Jobs, cron runs and email handlers query the primary. Without the
//! variable, or with replication off on the database, queries behave as
//! before: sessions reach the primary.
//!
//! # Free plan
//!
//! No extra cost: replication and sessions are free, and a replica answers
//! reads that would otherwise cross the world to the primary.
//!
//! # Examples
//!
//! ```
//! assert_eq!(ocre::replicas::REPLICAS_VAR, "D1_REPLICAS");
//! ```

use axum::http::{HeaderMap, HeaderValue, Method, header};
use cookie::Cookie;

/// Worker variable turning replicas on when set to `on`: `D1_REPLICAS`.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::replicas::REPLICAS_VAR, "D1_REPLICAS");
/// ```
pub const REPLICAS_VAR: &str = "D1_REPLICAS";

/// Seconds the bookmark cookie lives: long enough for the replicas to catch up.
pub(crate) const BOOKMARK_MAX_AGE: i64 = 300;

/// Whether the value of [`REPLICAS_VAR`] turns replicas on.
pub(crate) fn enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case("on"))
}

/// The cookie holding the bookmark of the D1 binding `binding`: `ocre_d1_db` for `DB`.
pub(crate) fn cookie_name(binding: &str) -> String {
    format!("ocre_d1_{}", binding.to_ascii_lowercase())
}

/// Where a request's session starts: the visitor's bookmark, else any
/// replica for reads (`GET`, `HEAD`) and the primary for the rest.
pub(crate) fn session_start(headers: &HeaderMap, method: &Method, binding: &str) -> String {
    let name = cookie_name(binding);
    let bookmark = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(Cookie::split_parse_encoded)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == name)
        .map(|cookie| cookie.value().to_owned())
        .filter(|bookmark| !bookmark.is_empty());
    bookmark.unwrap_or_else(|| {
        let read = *method == Method::GET || *method == Method::HEAD;
        (if read { "first-unconstrained" } else { "first-primary" }).to_owned()
    })
}

/// `Set-Cookie` value keeping `bookmark` for the visitor's next requests.
pub(crate) fn bookmark_cookie(binding: &str, bookmark: &str) -> Option<HeaderValue> {
    let cookie = Cookie::build((cookie_name(binding), bookmark.to_owned()))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(BOOKMARK_MAX_AGE));
    HeaderValue::from_str(&cookie.build().encoded().to_string()).ok()
}

#[cfg(test)]
#[path = "../tests/replicas.rs"]
mod tests;
