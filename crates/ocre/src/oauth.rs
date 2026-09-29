//! "Sign in with GitHub / Google": the OAuth 2.0 authorization code flow with PKCE.
//!
//! `ocre g auth --oauth github` (or `google`) generates the routes that use
//! this module (`src/oauth.rs` in the app): `GET /auth/github` redirects to
//! the provider with a random `state` and a PKCE challenge kept in the
//! session; `GET /auth/github/callback` checks the state, trades the code
//! for an access token ([`exchange_code`]), reads the user's [`Profile`]
//! ([`profile`]) and signs the matching user in.
//!
//! Each provider needs an OAuth app registered with it; its client id and
//! secret are Worker secrets named after the provider (`GITHUB_CLIENT_ID`,
//! `GITHUB_CLIENT_SECRET`, see [`Provider::client_id_secret`]), in
//! `.dev.vars` for `ocre dev` and uploaded with `ocre secrets push NAME --file .prod.vars`.
//!
//! # Free plan
//!
//! A sign-in makes 2 subrequests (Google) or 3 (GitHub: user and emails) out
//! of the 50 a free-plan request may make, and a few milliseconds of CPU; no
//! D1, KV or queue operation here (the app then reads and writes its users).
//!
//! ```
//! use ocre::oauth::{GITHUB, Pkce, authorize_url};
//!
//! let pkce = Pkce::new();
//! let state = ocre::token::generate();
//! let url = authorize_url(&GITHUB, "client-id", "https://app.example.com/auth/github/callback", &state, &pkce.challenge);
//! assert!(url.starts_with("https://github.com/login/oauth/authorize?response_type=code&client_id=client-id"));
//! ```

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

pub use crate::runtime::oauth::{exchange_code, profile};
use crate::{Error, Result};

/// An OAuth 2.0 provider: its endpoints and the scopes Ocre asks for.
///
/// Use [`GITHUB`] or [`GOOGLE`]; [`provider`] finds one by name.
///
/// # Examples
///
/// ```
/// let github = ocre::oauth::provider("github").unwrap();
/// assert_eq!(github.token_url, "https://github.com/login/oauth/access_token");
/// assert_eq!(github.client_id_secret, "GITHUB_CLIENT_ID");
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct Provider {
    /// Lowercase name, used in routes (`/auth/github`) and stored with identities.
    pub name: &'static str,
    /// Where the browser is sent to sign in and approve the app.
    pub authorize_url: &'static str,
    /// Where the Worker trades the code for an access token.
    pub token_url: &'static str,
    /// Where the Worker reads the signed-in user.
    pub userinfo_url: &'static str,
    /// Space-separated scopes: the user's identity and email address only.
    pub scopes: &'static str,
    /// Name of the Worker secret holding the OAuth app's client id.
    pub client_id_secret: &'static str,
    /// Name of the Worker secret holding the OAuth app's client secret.
    pub client_secret_secret: &'static str,
}

/// GitHub (OAuth app or GitHub App): scopes `read:user user:email`.
///
/// Register at <https://github.com/settings/developers> with the callback
/// URL `https://<your host>/auth/github/callback` (and
/// `http://localhost:8787/auth/github/callback` in a second app for
/// `ocre dev`).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::oauth::GITHUB.name, "github");
/// ```
pub const GITHUB: Provider = Provider {
    name: "github",
    authorize_url: "https://github.com/login/oauth/authorize",
    token_url: "https://github.com/login/oauth/access_token",
    userinfo_url: "https://api.github.com/user",
    scopes: "read:user user:email",
    client_id_secret: "GITHUB_CLIENT_ID",
    client_secret_secret: "GITHUB_CLIENT_SECRET",
};

/// Google (OpenID Connect): scopes `openid email profile`.
///
/// Create an OAuth client ID ("Web application") at
/// <https://console.cloud.google.com/apis/credentials> with the redirect URI
/// `https://<your host>/auth/google/callback` (and the `localhost:8787` one
/// for `ocre dev`).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::oauth::GOOGLE.scopes, "openid email profile");
/// ```
pub const GOOGLE: Provider = Provider {
    name: "google",
    authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo",
    scopes: "openid email profile",
    client_id_secret: "GOOGLE_CLIENT_ID",
    client_secret_secret: "GOOGLE_CLIENT_SECRET",
};

/// Every provider Ocre knows: `ocre g auth --oauth` accepts these names.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::oauth::PROVIDERS.len(), 2);
/// ```
pub const PROVIDERS: [&Provider; 2] = [&GITHUB, &GOOGLE];

/// The provider called `name` (`"github"`, `"google"`), if Ocre knows it.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::oauth::provider("google"), Some(&ocre::oauth::GOOGLE));
/// assert_eq!(ocre::oauth::provider("myspace"), None);
/// ```
pub fn provider(name: &str) -> Option<&'static Provider> {
    PROVIDERS.into_iter().find(|provider| provider.name == name)
}

/// A PKCE pair (RFC 7636, `S256`): keep the verifier in the session, send the challenge.
///
/// PKCE ties the code the provider returns to the browser session that
/// started the sign-in, so a stolen code is useless.
///
/// # Examples
///
/// ```
/// use ocre::oauth::Pkce;
///
/// let pkce = Pkce::new();
/// assert_eq!(pkce.verifier.len(), 43);
/// assert_eq!(Pkce::challenge_for(&pkce.verifier), pkce.challenge);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    /// The secret, sent with the token request ([`exchange_code`]).
    pub verifier: String,
    /// `BASE64URL(SHA-256(verifier))`, sent in the authorization URL.
    pub challenge: String,
}

impl Pkce {
    /// A new random verifier (32 bytes) and its challenge.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_ne!(ocre::oauth::Pkce::new(), ocre::oauth::Pkce::new());
    /// ```
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let verifier = crate::token::generate();
        let challenge = Self::challenge_for(&verifier);
        Self { verifier, challenge }
    }

    /// The `S256` challenge of `verifier`.
    ///
    /// # Examples
    ///
    /// ```
    /// // The example of RFC 7636, appendix B.
    /// let challenge = ocre::oauth::Pkce::challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    /// assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    /// ```
    pub fn challenge_for(verifier: &str) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    }
}

/// The URL to send the browser to: the provider's sign-in page for this app.
///
/// `redirect_uri` must be registered with the provider exactly;
/// `state` is a random value ([`crate::token::generate`]) kept in the
/// session and compared on the callback (CSRF protection);
/// `code_challenge` is [`Pkce::challenge`].
///
/// # Examples
///
/// ```
/// use ocre::oauth::{GOOGLE, authorize_url};
///
/// let url = authorize_url(&GOOGLE, "id", "http://localhost:8787/auth/google/callback", "st", "ch");
/// assert_eq!(
///     url,
///     "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id=id\
///      &redirect_uri=http%3A%2F%2Flocalhost%3A8787%2Fauth%2Fgoogle%2Fcallback&scope=openid+email+profile\
///      &state=st&code_challenge=ch&code_challenge_method=S256"
/// );
/// ```
pub fn authorize_url(
    provider: &Provider,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    let query = serde_urlencoded::to_string([
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("scope", provider.scopes),
        ("state", state),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256"),
    ])
    .expect("string pairs encode");
    format!("{}?{query}", provider.authorize_url)
}

/// The form body of the token request.
pub(crate) fn token_request_body(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> String {
    serde_urlencoded::to_string([
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("redirect_uri", redirect_uri),
        ("code", code),
        ("code_verifier", verifier),
    ])
    .expect("string pairs encode")
}

/// The access token of a token response, or 401 when the provider refused the code.
pub(crate) fn parse_token_response(provider: &Provider, status: u16, body: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: Option<String>,
        error: Option<String>,
        error_description: Option<String>,
    }
    let parsed: TokenResponse = serde_json::from_str(body).map_err(|err| {
        Error::internal(format!("{} token endpoint answered {status} with unexpected JSON: {err}", provider.name))
    })?;
    match parsed.access_token {
        Some(token) if (200..300).contains(&status) => Ok(token),
        _ => {
            let reason = parsed.error_description.or(parsed.error).unwrap_or_else(|| format!("status {status}"));
            crate::error::log_internal(&format!("{} refused the OAuth code: {reason}", provider.name));
            Err(Error::Unauthorized)
        }
    }
}

/// The user a provider signed in: its id there, and its email when verified.
///
/// # Examples
///
/// ```
/// let profile = ocre::oauth::Profile {
///     provider: "github",
///     uid: "583231".into(),
///     email: Some("octocat@github.com".into()),
///     name: Some("The Octocat".into()),
/// };
/// assert_eq!(profile.email.as_deref(), Some("octocat@github.com"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// The provider's [`Provider::name`].
    pub provider: &'static str,
    /// The user's stable id at the provider (GitHub's numeric id, Google's `sub`).
    pub uid: String,
    /// The user's email, only when the provider says it is verified; lowercased.
    pub email: Option<String>,
    /// The display name, when the user has one.
    pub name: Option<String>,
}

/// A profile from the provider's user JSON (and GitHub's `/user/emails` JSON).
pub(crate) fn parse_profile(provider: &'static Provider, user: &str, emails: Option<&str>) -> Result<Profile> {
    let bad = |err: serde_json::Error| Error::internal(format!("unexpected {} user JSON: {err}", provider.name));
    if provider == &GITHUB {
        #[derive(Deserialize)]
        struct User {
            id: i64,
            name: Option<String>,
            login: String,
        }
        #[derive(Deserialize)]
        struct Email {
            email: String,
            primary: bool,
            verified: bool,
        }
        let user: User = serde_json::from_str(user).map_err(bad)?;
        let emails: Vec<Email> = serde_json::from_str(emails.unwrap_or("[]")).map_err(bad)?;
        let email = emails.into_iter().find(|email| email.primary && email.verified).map(|email| email.email);
        Ok(Profile {
            provider: provider.name,
            uid: user.id.to_string(),
            email: email.map(|email| email.to_lowercase()),
            name: Some(user.name.unwrap_or(user.login)),
        })
    } else {
        #[derive(Deserialize)]
        struct User {
            sub: String,
            email: Option<String>,
            #[serde(default)]
            email_verified: bool,
            name: Option<String>,
        }
        let user: User = serde_json::from_str(user).map_err(bad)?;
        let email = user.email.filter(|_| user.email_verified).map(|email| email.to_lowercase());
        Ok(Profile { provider: provider.name, uid: user.sub, email, name: user.name })
    }
}

#[cfg(test)]
#[path = "../tests/oauth.rs"]
mod tests;
