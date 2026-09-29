//! Who is signed in, for HTML pages, with sessions tracked in D1. Generated
//! by `ocre g auth --db-sessions`.
//!
//! - `CurrentUser(user): CurrentUser` in a handler requires a signed-in user;
//!   other visitors are redirected to /login, then back to the page.
//! - `ConfirmedUser(user): ConfirmedUser` also requires a confirmed email.
//! - `OptionalUser(user): OptionalUser` gives `Option<User>` (e.g. a menu).
//! - `sign_in` / `sign_out` change the session; the login, sign-up,
//!   magic-link and OAuth handlers call them.
//!
//! Like Rails 8's generator: each sign-in is a row of the `user_sessions`
//! table (IP address, browser, last activity) and the encrypted session
//! cookie holds a random token whose digest identifies the row. Deleting the
//! row signs that device out on its next request: /account/sessions lists
//! them and signs out one or all others. Sessions last two weeks; the cookie
//! survives browser restarts only when "Remember me" was ticked.
//!
//! Cost: every request with a session reads the session and its user (2 D1
//! rows); the last activity is written at most once an hour per session.

// Extractors and helpers for the app's own pages; not all are used yet.
#![allow(dead_code)]

use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, Method, Uri, header, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use ocre::{Ctx, Error, Result, Session, security::url_from};

use crate::models::{
    user::User,
    user_session::{self, NewUserSession},
};

/// Session key holding the token of the signed-in session.
pub const SESSION_TOKEN: &str = "session_token";
/// Session key holding the page to return to after logging in.
const RETURN_TO: &str = "return_to";
/// How long a sign-in lasts, remembered or not.
pub const SESSION_SECONDS: i64 = 14 * 24 * 3600;
/// OAuth providers offered on the login page (`ocre g auth --oauth github`).
pub const OAUTH_PROVIDERS: &[&str] = &[];

/// The signed-in user. Visitors are redirected to /login.
pub struct CurrentUser(pub User);

/// The signed-in user, if any.
pub struct OptionalUser(pub Option<User>);

/// The signed-in user with a confirmed email. Visitors are redirected to
/// /login, unconfirmed users to /account (which offers a new email).
pub struct ConfirmedUser(pub User);

impl FromRequestParts<Ctx> for OptionalUser {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, ctx: &Ctx) -> Result<Self> {
        let session = Session::from_request_parts(parts, ctx).await?;
        let Some(token) = session.get::<String>(SESSION_TOKEN)? else { return Ok(Self(None)) };
        let user = user_session::authenticate(ctx, &token).await?;
        if user.is_none() {
            // Signed out from another device: forget the token.
            session.remove(SESSION_TOKEN)?;
        }
        Ok(Self(user))
    }
}

impl FromRequestParts<Ctx> for CurrentUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, ctx: &Ctx) -> std::result::Result<Self, Response> {
        match OptionalUser::from_request_parts(parts, ctx).await {
            Ok(OptionalUser(Some(user))) => Ok(Self(user)),
            Ok(OptionalUser(None)) => Err(to_login(parts, ctx).await.unwrap_or_else(IntoResponse::into_response)),
            Err(err) => Err(err.into_response()),
        }
    }
}

impl FromRequestParts<Ctx> for ConfirmedUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, ctx: &Ctx) -> std::result::Result<Self, Response> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, ctx).await?;
        if user.confirmed() {
            return Ok(Self(user));
        }
        let session = Session::from_request_parts(parts, ctx).await.map_err(IntoResponse::into_response)?;
        session.flash("alert", "Please confirm your email address first.").map_err(IntoResponse::into_response)?;
        Err(Redirect::to("/account").into_response())
    }
}

/// Redirects to /login, remembering the page for GET requests (a form
/// submission cannot be replayed after a redirect).
async fn to_login(parts: &mut Parts, ctx: &Ctx) -> Result<Response> {
    let session = Session::from_request_parts(parts, ctx).await?;
    if parts.method == Method::GET {
        session.insert(RETURN_TO, parts.uri.path_and_query().map_or("/", |path| path.as_str()))?;
    }
    session.flash("alert", "Please log in to continue.")?;
    Ok(Redirect::to("/login").into_response())
}

/// Signs `user` in and returns where to go next: the page that asked for a
/// login, or `/`. Records the session (IP address and browser from
/// `headers`) and empties the cookie session first (pending flash messages
/// stay), so nothing from before the login carries over (session fixation).
/// `remember` keeps the cookie across browser restarts.
pub async fn sign_in(ctx: &Ctx, session: &Session, headers: &HeaderMap, user: &User, remember: bool) -> Result<String> {
    // Only paths of this app: never an open redirect.
    let return_to = session.get::<String>(RETURN_TO)?.and_then(|path| url_from(&Uri::default(), &path));
    let new = NewUserSession {
        user_id: user.id,
        ip_address: ocre::remote_ip(headers).map(|ip| ip.to_string()),
        user_agent: headers.get(header::USER_AGENT).and_then(|agent| agent.to_str().ok()).map(str::to_owned),
        seconds: SESSION_SECONDS,
    };
    let token = user_session::start(ctx, new).await?;
    session.clear()?;
    session.insert(SESSION_TOKEN, token)?;
    if remember { session.remember_for(SESSION_SECONDS)? } else { session.expire_in(SESSION_SECONDS)? }
    Ok(return_to.unwrap_or_else(|| "/".to_owned()))
}

/// Signs this device out: deletes its session row and empties the session.
pub async fn sign_out(ctx: &Ctx, session: &Session) -> Result<()> {
    if let Some(token) = session.get::<String>(SESSION_TOKEN)? {
        user_session::end(ctx, &token).await?;
    }
    session.clear()
}

/// The current session's token, for /account/sessions.
pub fn session_token(session: &Session) -> Result<Option<String>> {
    session.get(SESSION_TOKEN)
}

/// `https://app.example.com`: scheme and host of the current request, for
/// links in emails. Cloudflare routes requests by host name, so this is always
/// one of the app's own hosts (list them in ALLOWED_HOSTS to be sure).
pub fn origin(uri: &Uri) -> String {
    let scheme = uri.scheme_str().unwrap_or("https");
    let host = uri.authority().map_or("localhost", |authority| authority.as_str());
    format!("{scheme}://{host}")
}
