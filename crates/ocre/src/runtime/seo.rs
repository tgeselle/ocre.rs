//! [`redirect_www`], [`indexnow_routes`] and [`submit_to_indexnow`]: the parts of `ocre::seo` that read the Worker's settings.

use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};

use super::Ctx;
use crate::{
    Error, Result, http,
    mail::APP_URL,
    seo::{INDEXNOW_ENDPOINT, INDEXNOW_KEY, INDEXNOW_KEY_PATH, indexnow_body, valid_indexnow_key, www_target},
};

fn var(ctx: &Ctx, name: &str) -> Option<String> {
    ctx.env().var(name).ok().map(|value| value.to_string().trim().to_owned()).filter(|value| !value.is_empty())
}

/// Middleware sending `www.` + the host of `APP_URL` to `APP_URL`, same path and query, with `301 Moved Permanently`.
///
/// For an app answering on both `example.com` and `www.example.com`
/// (two custom domains of the Worker): search engines and links then see
/// one address per page. Requests to any other host, or without `APP_URL`,
/// go through unchanged. Add it with
/// `.layer(axum::middleware::from_fn(ocre::seo::redirect_www))` in
/// `routes()`. Reads one variable; no billed resource.
///
/// # Examples
///
/// ```
/// use axum::{Router, middleware::from_fn, routing::get};
/// use ocre::Ctx;
///
/// fn routes() -> Router<Ctx> {
///     Router::new().route("/", get(|| async { "home" })).layer(from_fn(ocre::seo::redirect_www))
/// }
/// # let _ = routes;
/// ```
pub async fn redirect_www(request: Request, next: Next) -> Response {
    let Some(ctx) = request.extensions().get::<Ctx>().cloned() else { return next.run(request).await };
    let host = request.headers().get(header::HOST).and_then(|value| value.to_str().ok()).or(request.uri().host());
    let path = request.uri().path_and_query().map_or("/", |path| path.as_str());
    match var(&ctx, APP_URL).and_then(|base| www_target(&base, host, path)) {
        Some(target) => (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, target)]).into_response(),
        None => next.run(request).await,
    }
}

/// `GET /indexnow.txt`: the [`INDEXNOW_KEY`](crate::seo::INDEXNOW_KEY), which search engines fetch to check a submission.
///
/// 404 when the variable is not set. Merge it into `routes()`:
/// `.merge(ocre::seo::indexnow_routes())`.
pub fn indexnow_routes() -> Router<Ctx> {
    Router::new().route(
        INDEXNOW_KEY_PATH,
        get(|State(ctx): State<Ctx>| async move {
            match var(&ctx, INDEXNOW_KEY) {
                Some(key) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], key).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }),
    )
}

/// Tells IndexNow-participating search engines (Bing, Yandex, Seznam, Naver...) that `urls` were added or changed.
///
/// One `POST` to [`INDEXNOW_ENDPOINT`](crate::seo::INDEXNOW_ENDPOINT)
/// with the URLs (absolute, on `APP_URL`'s host, at most 10,000), the
/// [`INDEXNOW_KEY`](crate::seo::INDEXNOW_KEY) and where to check it
/// ([`indexnow_routes`]). Call it after publishing (a deploy script, a
/// job): engines may ignore sites that submit unchanged pages often.
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) when `APP_URL` or a valid
/// `INDEXNOW_KEY` is missing, or IndexNow refuses the submission (it
/// answers 200 or 202 when accepted).
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Result, seo};
///
/// async fn published(ctx: &Ctx, slug: &str) -> Result<()> {
///     let url = ocre::mail::url(ctx, &format!("/blog/{slug}"))?;
///     seo::submit_to_indexnow(ctx, &[url]).await
/// }
/// ```
pub fn submit_to_indexnow(ctx: &Ctx, urls: &[String]) -> impl Future<Output = Result<()>> + Send + use<> {
    let request = (|| {
        let base =
            var(ctx, APP_URL).ok_or_else(|| Error::internal("IndexNow needs APP_URL, the site's public origin"))?;
        let key = var(ctx, INDEXNOW_KEY).filter(|key| valid_indexnow_key(key)).ok_or_else(|| {
            Error::internal(
                "IndexNow needs INDEXNOW_KEY: 8 to 128 letters, digits or `-` (e.g. from `ocre secret`), in worker.env",
            )
        })?;
        let body =
            indexnow_body(&base, &key, urls).ok_or_else(|| Error::internal("APP_URL is not an http(s) origin"))?;
        Ok::<_, Error>(http::Request::post(INDEXNOW_ENDPOINT).json(&body))
    })();
    async move {
        request?.send().await?.error_for_status()?;
        Ok(())
    }
}
