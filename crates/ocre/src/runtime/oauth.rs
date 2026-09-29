use wasm_bindgen::JsValue;
use worker::{Env, Fetch, Headers, Method, Request, RequestInit, send::SendFuture};

use crate::{
    Ctx, Error, Result,
    oauth::{GITHUB, Profile, Provider, parse_profile, parse_token_response, token_request_body},
};

/// GitHub's API refuses requests without a `User-Agent`.
const USER_AGENT: &str = "ocre (https://github.com/tgeselle/ocre.rs)";

/// Trades the authorization `code` from the callback for an access token (one subrequest).
///
/// Sends the client id and secret from the provider's Worker secrets
/// ([`Provider::client_id_secret`], [`Provider::client_secret_secret`]),
/// the same `redirect_uri` as the authorization URL, and the PKCE
/// `verifier` kept in the session.
///
/// # Errors
///
/// - [`Error::Unauthorized`] when the provider refuses the code (expired,
///   used twice, wrong verifier); the reason is logged.
/// - [`Error::Internal`] when a secret is missing (the message names it) or
///   the provider cannot be reached.
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, oauth::{GITHUB, exchange_code, profile}};
///
/// async fn callback(ctx: &Ctx, code: &str, verifier: &str) -> Result<String> {
///     let token = exchange_code(ctx, &GITHUB, "https://app.example.com/auth/github/callback", code, verifier).await?;
///     Ok(profile(&GITHUB, &token).await?.uid)
/// }
/// # let _ = callback;
/// ```
pub fn exchange_code(
    ctx: &Ctx,
    provider: &'static Provider,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> impl Future<Output = Result<String>> + Send + use<> {
    let env = ctx.env().clone();
    let (redirect_uri, code, verifier) = (redirect_uri.to_owned(), code.to_owned(), verifier.to_owned());
    SendFuture::new(async move {
        let client_id = secret(&env, provider.client_id_secret)?;
        let client_secret = secret(&env, provider.client_secret_secret)?;
        let body = token_request_body(&client_id, &client_secret, &redirect_uri, &code, &verifier);
        let headers = Headers::new();
        headers.set("Accept", "application/json")?;
        headers.set("Content-Type", "application/x-www-form-urlencoded")?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_headers(headers).with_body(Some(JsValue::from_str(&body)));
        let mut response = Fetch::Request(Request::new_with_init(provider.token_url, &init)?).send().await?;
        let status = response.status_code();
        parse_token_response(provider, status, &response.text().await?)
    })
}

/// Reads the signed-in user's [`Profile`] with an access token (one subrequest; two for GitHub).
///
/// The email is kept only when the provider marks it verified (GitHub: the
/// primary verified address of `/user/emails`; Google: `email_verified`).
///
/// # Errors
///
/// [`Error::Internal`] when the provider cannot be reached or answers
/// something unexpected (a revoked token included).
///
/// # Examples
///
/// ```no_run
/// use ocre::{Result, oauth::{GOOGLE, profile}};
///
/// async fn email(token: &str) -> Result<Option<String>> {
///     Ok(profile(&GOOGLE, token).await?.email)
/// }
/// # let _ = email;
/// ```
pub fn profile(
    provider: &'static Provider,
    access_token: &str,
) -> impl Future<Output = Result<Profile>> + Send + use<> {
    let access_token = access_token.to_owned();
    SendFuture::new(async move {
        let user = get_json(provider, provider.userinfo_url, &access_token).await?;
        let emails = if provider == &GITHUB {
            Some(get_json(provider, "https://api.github.com/user/emails", &access_token).await?)
        } else {
            None
        };
        parse_profile(provider, &user, emails.as_deref())
    })
}

async fn get_json(provider: &Provider, url: &str, access_token: &str) -> Result<String> {
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {access_token}"))?;
    headers.set("Accept", "application/json")?;
    headers.set("User-Agent", USER_AGENT)?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers);
    let mut response = Fetch::Request(Request::new_with_init(url, &init)?).send().await?;
    let (status, body) = (response.status_code(), response.text().await?);
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(Error::internal(format!("{} answered {status} for {url}: {body}", provider.name)))
    }
}

fn secret(env: &Env, name: &str) -> Result<String> {
    env.secret(name).map(|secret| secret.to_string()).map_err(|_| {
        Error::internal(format!(
            "the {name} secret is not set. Fix: put it in .dev.vars for `ocre dev` and run \
             `ocre secrets push {name} --file .prod.vars` for production"
        ))
    })
}
