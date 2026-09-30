//! The request's cookies as an extractor, plain, signed or encrypted
//! (Rails' `cookies`, `cookies.signed` and `cookies.encrypted`).

use std::{
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, HeaderValue, header, request::Parts},
};
use cookie::{Cookie, CookieJar, SameSite};

use crate::{
    Error, Result,
    session::{Keys, Rejection, SESSION_COOKIE, reject},
};

/// The request's cookies; what handlers set goes out with the response.
///
/// - [`get`](Self::get) / [`set`](Self::set): plain values the browser can read and change.
/// - [`signed`](Self::signed) / [`set_signed`](Self::set_signed): readable,
///   but a changed value reads as `None` (HMAC-SHA256).
/// - [`encrypted`](Self::encrypted) / [`set_encrypted`](Self::set_encrypted):
///   hidden and tamper-proof (AES-256-GCM).
///
/// Signed and encrypted cookies use keys derived from `SECRET_KEY_BASE`;
/// values made with a key of `SECRET_KEY_BASE_PREVIOUS` still read. Cookies
/// Ocre sets are `HttpOnly`, `SameSite=Lax`, `Path=/`, and `Secure` over
/// HTTPS. For per-visitor state prefer the [`Session`](crate::Session),
/// which is encrypted too; use cookies for values that outlive it or that
/// another part of the site reads (a theme, a "remember me" token).
///
/// # Free plan
///
/// Nothing billed: cookies travel with the requests.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
/// use ocre::{Cookies, Result};
///
/// async fn theme(cookies: Cookies) -> Result<String> {
///     let theme = cookies.get("theme").unwrap_or_else(|| "light".to_owned());
///     cookies.set_signed("seen_banner", "1", Some(Duration::from_secs(30 * 86_400)))?;
///     Ok(theme)
/// }
/// # let _ = theme;
/// ```
#[derive(Clone)]
pub struct Cookies(Arc<Mutex<State>>);

struct State {
    jar: CookieJar,
    keys: std::result::Result<Keys, String>,
    secure: bool,
}

impl Cookies {
    /// The cookies of a request; `secure` marks those set over HTTPS.
    pub(crate) fn from_headers(headers: &HeaderMap, keys: std::result::Result<Keys, String>, secure: bool) -> Self {
        let mut jar = CookieJar::new();
        let cookies = headers.get_all(header::COOKIE).iter().filter_map(|value| value.to_str().ok());
        for cookie in cookies.flat_map(Cookie::split_parse_encoded).filter_map(std::result::Result::ok) {
            jar.add_original(cookie.into_owned());
        }
        Self(Arc::new(Mutex::new(State { jar, keys, secure })))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The value of the plain cookie `name`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// async fn theme(cookies: ocre::Cookies) -> String {
    ///     cookies.get("theme").unwrap_or_default()
    /// }
    /// # let _ = theme;
    /// ```
    pub fn get(&self, name: &str) -> Option<String> {
        self.state().jar.get(name).map(|cookie| cookie.value().to_owned())
    }

    /// The value of the signed cookie `name`; `None` when absent or changed by the client.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when `SECRET_KEY_BASE` is missing or shorter than 64 characters.
    pub fn signed(&self, name: &str) -> Result<Option<String>> {
        let state = self.state();
        let keys = keys(&state)?;
        Ok(keys.iter().find_map(|key| state.jar.signed(key).get(name)).map(|cookie| cookie.value().to_owned()))
    }

    /// The value of the encrypted cookie `name`; `None` when absent or changed by the client.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when `SECRET_KEY_BASE` is missing or shorter than 64 characters.
    pub fn encrypted(&self, name: &str) -> Result<Option<String>> {
        let state = self.state();
        let keys = keys(&state)?;
        Ok(keys.iter().find_map(|key| state.jar.private(key).get(name)).map(|cookie| cookie.value().to_owned()))
    }

    /// Sets a plain cookie; `max_age` `None` makes it last until the browser closes.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] for the session cookie's name, which Ocre manages.
    pub fn set(&self, name: &str, value: &str, max_age: Option<Duration>) -> Result<()> {
        let cookie = self.build(name, value, max_age)?;
        self.state().jar.add(cookie);
        Ok(())
    }

    /// Sets a signed cookie: the browser sees the value, and a changed one reads as `None`.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] for the session cookie's name, or when
    /// `SECRET_KEY_BASE` is missing or shorter than 64 characters.
    pub fn set_signed(&self, name: &str, value: &str, max_age: Option<Duration>) -> Result<()> {
        let cookie = self.build(name, value, max_age)?;
        let mut state = self.state();
        let key = keys(&state)?[0].clone();
        state.jar.signed_mut(&key).add(cookie);
        Ok(())
    }

    /// Sets an encrypted cookie: the browser can neither read nor change the value.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] for the session cookie's name, or when
    /// `SECRET_KEY_BASE` is missing or shorter than 64 characters.
    pub fn set_encrypted(&self, name: &str, value: &str, max_age: Option<Duration>) -> Result<()> {
        let cookie = self.build(name, value, max_age)?;
        let mut state = self.state();
        let key = keys(&state)?[0].clone();
        state.jar.private_mut(&key).add(cookie);
        Ok(())
    }

    /// Deletes the cookie `name` from the browser.
    pub fn remove(&self, name: &str) {
        // Not `CookieJar::remove`: its removal cookie reads the clock, which
        // panics on wasm32-unknown-unknown. Max-Age=0 and a past date do the same.
        let mut cookie = Cookie::build((name.to_owned(), "")).path("/").build();
        cookie.set_max_age(cookie::time::Duration::ZERO);
        cookie.set_expires(cookie::time::OffsetDateTime::UNIX_EPOCH);
        self.state().jar.add(cookie);
    }

    fn build(&self, name: &str, value: &str, max_age: Option<Duration>) -> Result<Cookie<'static>> {
        if name == SESSION_COOKIE {
            return Err(Error::internal(format!(
                "`{SESSION_COOKIE}` is the session cookie. Fix: store the value in the `Session`, or pick another cookie name"
            )));
        }
        let mut cookie = Cookie::build((name.to_owned(), value.to_owned()))
            .path("/")
            .http_only(true)
            .same_site(SameSite::Lax)
            .secure(self.state().secure)
            .build();
        if let Some(max_age) = max_age {
            cookie.set_max_age(cookie::time::Duration::seconds(i64::try_from(max_age.as_secs()).unwrap_or(i64::MAX)));
        }
        Ok(cookie)
    }

    /// `Set-Cookie` values of the cookies set during the request.
    pub(crate) fn set_cookies(&self) -> Vec<HeaderValue> {
        let state = self.state();
        let values = state.jar.delta().map(|cookie| HeaderValue::from_str(&cookie.encoded().to_string()));
        values.filter_map(std::result::Result::ok).collect()
    }
}

/// The current key, then the previous ones.
fn keys(state: &State) -> Result<Vec<cookie::Key>> {
    let keys = state.keys.as_ref().map_err(|err| Error::internal(err.clone()))?;
    Ok(std::iter::once(&keys.current).chain(&keys.previous).cloned().collect())
}

impl<S: Sync> FromRequestParts<S> for Cookies {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> std::result::Result<Self, Rejection> {
        parts.extensions.get::<Cookies>().cloned().ok_or_else(|| {
            reject(Error::internal(
                "no cookies on this request. Fix: serve the app with `ocre::serve`, which adds them",
            ))
        })
    }
}

#[cfg(test)]
#[path = "../tests/cookies.rs"]
mod tests;
