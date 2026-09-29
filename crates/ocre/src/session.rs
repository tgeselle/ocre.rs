//! Sessions stored in an encrypted cookie, like Rails' default cookie store,
//! plus flash messages.
//!
//! The cookie is encrypted and authenticated with AES-256-GCM using a key
//! derived from the `SECRET_KEY_BASE` Worker secret, so clients can neither
//! read nor change it. Nothing is stored on the server: sessions cost no D1
//! rows and no KV operations. Browsers cap a cookie at 4 KB, so keep ids in
//! the session, not records.

use std::sync::{Arc, Mutex, MutexGuard};

use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, HeaderValue, header, request::Parts},
};
use cookie::{Cookie, CookieJar, Key, SameSite};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::{Error, Result};

/// Extractor failures render as HTML pages in full-stack apps and as JSON in
/// API-only apps.
#[cfg(feature = "html")]
pub(crate) type Rejection = Error;
#[cfg(not(feature = "html"))]
pub(crate) type Rejection = crate::ApiError;

#[cfg(feature = "html")]
pub(crate) fn reject(err: Error) -> Rejection {
    err
}

#[cfg(not(feature = "html"))]
pub(crate) fn reject(err: Error) -> Rejection {
    crate::ApiError::from(err)
}

/// Name of the session cookie: `_ocre_session`.
///
/// The cookie is `HttpOnly`, `SameSite=Lax`, `Path=/`, and `Secure` on HTTPS
/// requests.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::SESSION_COOKIE, "_ocre_session");
/// ```
pub const SESSION_COOKIE: &str = "_ocre_session";
/// Name of the Worker secret the session encryption key is derived from: `SECRET_KEY_BASE`.
///
/// It must be at least 64 characters. `ocre secret` generates one, `ocre new`
/// writes a local one to `.dev.vars`, and `ocre deploy` uploads the production
/// one. Without it, requests that read an existing session cookie or change
/// the session fail with [`Error::Internal`] naming this fix. [`jwt`](crate::jwt)
/// derives its signing key from the same secret.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::State;
/// use ocre::{Ctx, Result, SECRET_KEY_BASE};
///
/// async fn secret_is_set(State(ctx): State<Ctx>) -> Result<String> {
///     Ok(ctx.env().secret(SECRET_KEY_BASE).is_ok().to_string())
/// }
/// # let _ = secret_is_set;
/// ```
pub const SECRET_KEY_BASE: &str = "SECRET_KEY_BASE";
/// Browsers drop cookies larger than this (name, value and attributes).
const MAX_COOKIE_BYTES: usize = 4096;
/// Session entry holding the flash messages for the next request.
const FLASH_KEY: &str = "_flash";

/// Derives the cookie encryption key from `SECRET_KEY_BASE`.
pub(crate) fn key_from_secret(secret: Option<String>) -> std::result::Result<Key, String> {
    checked_secret(secret).map(|secret| Key::derive_from(secret.as_bytes()))
}

/// `SECRET_KEY_BASE` when it is set and long enough, or an error naming the fix.
pub(crate) fn checked_secret(secret: Option<String>) -> std::result::Result<String, String> {
    let fix = "Fix: run `ocre secret`, put the value in .dev.vars as SECRET_KEY_BASE=... for `ocre dev` \
               (`ocre new` does this), and deploy with `ocre deploy`, which uploads it";
    match secret {
        None => Err(format!("the {SECRET_KEY_BASE} secret is not set. {fix}")),
        Some(secret) if secret.len() < 64 => Err(format!("{SECRET_KEY_BASE} is shorter than 64 characters. {fix}")),
        Some(secret) => Ok(secret),
    }
}

/// The current request's session, stored in an encrypted cookie, as an extractor.
///
/// Like Rails' default cookie store: values are serialized as JSON and the
/// cookie is encrypted and authenticated with AES-256-GCM using a key derived
/// from [`SECRET_KEY_BASE`], so clients can neither read nor change it. A
/// cookie that does not decrypt (tampered, or encrypted with an older key)
/// starts an empty session. The cookie is decrypted on first use; changes are
/// sent back as one `Set-Cookie` header when the handler returns, and only if
/// something changed. Clones share the same session.
///
/// Browsers drop cookies over 4 KB: store ids, not records. A response whose
/// session cookie would be larger becomes a 500 (logged, naming that fix).
///
/// Added by [`serve`](crate::serve). Outside `serve` the extractor rejects
/// with [`Error::Internal`]: as an HTML page with the `html` feature, as
/// [`ApiError`](crate::ApiError) JSON without it.
///
/// # Free plan
///
/// Nothing is stored on the server: sessions cost no D1 rows and no KV
/// operations, only a little CPU for AES-GCM.
///
/// # Examples
///
/// ```no_run
/// use axum::response::Redirect;
/// use ocre::{Result, Session};
///
/// async fn login(session: Session) -> Result<Redirect> {
///     session.insert("user_id", 42)?;
///     session.flash("notice", "Signed in.")?;
///     Ok(Redirect::to("/"))
/// }
///
/// async fn current_user_id(session: Session) -> Result<String> {
///     let id: Option<i64> = session.get("user_id")?;
///     Ok(id.map_or("guest".to_owned(), |id| id.to_string()))
/// }
/// # let _ = (login, current_user_id);
/// ```
#[derive(Clone)]
pub struct Session(Arc<Mutex<State>>);

struct State {
    key: std::result::Result<Key, String>,
    /// Encrypted cookie value from the request, decrypted on first use.
    cookie: Option<String>,
    loaded: bool,
    data: Map<String, Value>,
    /// Flash messages that arrived with this request.
    flash: Map<String, Value>,
    changed: bool,
    secure: bool,
}

impl State {
    fn key(&self) -> Result<&Key> {
        self.key.as_ref().map_err(|err| Error::internal(err.clone()))
    }
}

impl Session {
    /// Session for a request with these headers. `secure` marks the cookie
    /// `Secure` (HTTPS requests).
    pub(crate) fn from_headers(headers: &HeaderMap, key: std::result::Result<Key, String>, secure: bool) -> Self {
        let cookie = headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(Cookie::split_parse_encoded)
            .filter_map(std::result::Result::ok)
            .find(|cookie| cookie.name() == SESSION_COOKIE)
            .map(|cookie| cookie.value().to_owned());
        Self(Arc::new(Mutex::new(State {
            key,
            cookie,
            loaded: false,
            data: Map::new(),
            flash: Map::new(),
            changed: false,
            secure,
        })))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock already fails the request.
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The state with the cookie decrypted. A cookie that does not decrypt
    /// (tampered, or encrypted with an older key) starts an empty session.
    fn loaded(&self) -> Result<MutexGuard<'_, State>> {
        let mut state = self.state();
        if !state.loaded {
            if let Some(value) = state.cookie.take() {
                let mut jar = CookieJar::new();
                jar.add_original(Cookie::new(SESSION_COOKIE, value));
                let cookie = jar.private(state.key()?).get(SESSION_COOKIE);
                state.data = cookie.and_then(|cookie| serde_json::from_str(cookie.value()).ok()).unwrap_or_default();
            }
            if let Some(Value::Object(flash)) = state.data.remove(FLASH_KEY) {
                state.flash = flash;
                state.changed = true;
            }
            state.loaded = true;
        }
        Ok(state)
    }

    /// The loaded state, marked as changed. Fails without a key, before any change.
    fn writable(&self) -> Result<MutexGuard<'_, State>> {
        let mut state = self.loaded()?;
        state.key()?;
        state.changed = true;
        Ok(state)
    }

    /// Returns the value stored under `key`, or `None` if absent or not deserializable as `T`.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when the request carries a session cookie but
    /// [`SECRET_KEY_BASE`] is missing or shorter than 64 characters.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ocre::{Error, Result, Session};
    ///
    /// async fn dashboard(session: Session) -> Result<String> {
    ///     let user_id: i64 = session.get("user_id")?.ok_or(Error::Unauthorized)?;
    ///     Ok(format!("user {user_id}"))
    /// }
    /// # let _ = dashboard;
    /// ```
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        Ok(self.loaded()?.data.get(key).and_then(|value| T::deserialize(value).ok()))
    }

    /// Stores `value` under `key`, replacing any previous value.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when `value` does not serialize to JSON, or when
    /// [`SECRET_KEY_BASE`] is missing or shorter than 64 characters (nothing is
    /// changed then).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ocre::{Result, Session};
    ///
    /// async fn set_theme(session: Session) -> Result<()> {
    ///     session.insert("theme", "dark")
    /// }
    /// # let _ = set_theme;
    /// ```
    pub fn insert(&self, key: &str, value: impl Serialize) -> Result<()> {
        self.insert_json(key, serde_json::to_value(value))
    }

    fn insert_json(&self, key: &str, value: serde_json::Result<Value>) -> Result<()> {
        let value =
            value.map_err(|err| Error::internal(format!("session value for `{key}` is not serializable: {err}")))?;
        self.writable()?.data.insert(key.to_owned(), value);
        Ok(())
    }

    /// Removes `key` and returns whether it was there.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when [`SECRET_KEY_BASE`] is missing or shorter than
    /// 64 characters.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ocre::{Result, Session};
    ///
    /// async fn reset_theme(session: Session) -> Result<String> {
    ///     Ok(if session.remove("theme")? { "reset" } else { "already default" }.to_owned())
    /// }
    /// # let _ = reset_theme;
    /// ```
    pub fn remove(&self, key: &str) -> Result<bool> {
        Ok(self.writable()?.data.remove(key).is_some())
    }

    /// Empties the session (sign out); flash messages set during this request are kept.
    ///
    /// The response then deletes the cookie if nothing is left.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when [`SECRET_KEY_BASE`] is missing or shorter than
    /// 64 characters.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::response::Redirect;
    /// use ocre::{Result, Session};
    ///
    /// async fn logout(session: Session) -> Result<Redirect> {
    ///     session.clear()?;
    ///     session.flash("notice", "Signed out.")?;
    ///     Ok(Redirect::to("/"))
    /// }
    /// # let _ = logout;
    /// ```
    pub fn clear(&self) -> Result<()> {
        self.writable()?.data.retain(|key, _| key == FLASH_KEY);
        Ok(())
    }

    /// Stores `message` under `kind` to show on the next request, usually after a redirect.
    ///
    /// Kinds are free-form; generated code uses `notice` and `alert`. A second
    /// message of the same kind replaces the first. The next request reads it
    /// with [`Flash`] (or [`flashes`](Self::flashes)), after which it is gone.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when [`SECRET_KEY_BASE`] is missing or shorter than
    /// 64 characters.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::response::Redirect;
    /// use ocre::{Result, Session};
    ///
    /// async fn create(session: Session) -> Result<Redirect> {
    ///     session.flash("notice", "Post was successfully created.")?;
    ///     Ok(Redirect::to("/posts"))
    /// }
    /// # let _ = create;
    /// ```
    pub fn flash(&self, kind: &str, message: impl Into<String>) -> Result<()> {
        self.flash_message(kind, message.into())
    }

    fn flash_message(&self, kind: &str, message: String) -> Result<()> {
        let mut state = self.writable()?;
        let flash = state.data.entry(FLASH_KEY).or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(flash) = flash {
            flash.insert(kind.to_owned(), Value::String(message));
        }
        Ok(())
    }

    /// Returns the flash messages set by the previous request and removes them from the session.
    ///
    /// The [`Flash`] extractor calls it.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when the request carries a session cookie but
    /// [`SECRET_KEY_BASE`] is missing or shorter than 64 characters.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ocre::{Result, Session};
    ///
    /// async fn index(session: Session) -> Result<String> {
    ///     Ok(session.flashes()?.notice().unwrap_or_default().to_owned())
    /// }
    /// # let _ = index;
    /// ```
    pub fn flashes(&self) -> Result<Flash> {
        let state = self.loaded()?;
        Ok(Flash(state.flash.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect()))
    }

    /// `Set-Cookie` value when the session changed during the request.
    pub(crate) fn set_cookie(&self) -> Result<Option<HeaderValue>> {
        let state = self.state();
        if !state.changed {
            return Ok(None);
        }
        let (key, data) = (state.key()?, &state.data);
        let mut cookie = if data.is_empty() {
            // Not `make_removal`: it reads the clock, which panics on
            // wasm32-unknown-unknown. A past date and Max-Age=0 do the same.
            let mut cookie = Cookie::from(SESSION_COOKIE);
            cookie.set_max_age(cookie::time::Duration::ZERO);
            cookie.set_expires(cookie::time::OffsetDateTime::UNIX_EPOCH);
            cookie
        } else {
            let json = serde_json::to_string(data).expect("JSON values serialize");
            let mut jar = CookieJar::new();
            jar.private_mut(key).add(Cookie::new(SESSION_COOKIE, json));
            jar.get(SESSION_COOKIE).expect("just added").clone()
        };
        cookie.set_path("/");
        cookie.set_http_only(true);
        cookie.set_same_site(SameSite::Lax);
        cookie.set_secure(state.secure);
        let header = cookie.encoded().to_string();
        if header.len() > MAX_COOKIE_BYTES {
            return Err(Error::internal(format!(
                "the session cookie would be {} bytes; browsers drop cookies over {MAX_COOKIE_BYTES}. Fix: store ids in the session, not records",
                header.len()
            )));
        }
        Ok(Some(HeaderValue::from_str(&header).expect("encoded cookies are valid header values")))
    }
}

impl<S: Sync> FromRequestParts<S> for Session {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> std::result::Result<Self, Rejection> {
        parts.extensions.get::<Session>().cloned().ok_or_else(|| {
            reject(Error::internal("no session on this request. Fix: serve the app with `ocre::serve`, which adds it"))
        })
    }
}

/// Flash messages set by the previous request, as an extractor.
///
/// Extracting them removes them from the session, so each message shows once
/// (Rails' `flash`). Rejects like [`Session`] when used outside
/// [`serve`](crate::serve) or when the secret is missing.
///
/// In templates: `{% if let Some(notice) = flash.notice() %}<p>{{ notice }}</p>{% endif %}`.
///
/// # Examples
///
/// ```no_run
/// use ocre::Flash;
///
/// async fn index(flash: Flash) -> String {
///     flash.iter().map(|(kind, message)| format!("{kind}: {message}\n")).collect()
/// }
/// # let _ = index;
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flash(Vec<(String, String)>);

impl Flash {
    /// The message of the given `kind`, if the previous request set one.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::Flash::default().get("warning"), None);
    /// ```
    pub fn get(&self, kind: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == kind).map(|(_, message)| message.as_str())
    }

    /// The `notice` message (success), same as `flash.get("notice")`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::Flash::default().notice(), None);
    /// ```
    pub fn notice(&self) -> Option<&str> {
        self.get("notice")
    }

    /// The `alert` message (failure), same as `flash.get("alert")`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::Flash::default().alert(), None);
    /// ```
    pub fn alert(&self) -> Option<&str> {
        self.get("alert")
    }

    /// Every `(kind, message)` pair.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::Flash::default().iter().count(), 0);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(kind, message)| (kind.as_str(), message.as_str()))
    }

    /// Whether there is no message.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(ocre::Flash::default().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<S: Sync> FromRequestParts<S> for Flash {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> std::result::Result<Self, Rejection> {
        Session::from_request_parts(parts, state).await?.flashes().map_err(reject)
    }
}

#[cfg(test)]
#[path = "../tests/session.rs"]
mod tests;
