//! `AllowBrowser`: turns away outdated browsers with `406 Not Acceptable` (Rails' `allow_browser`).

use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use tower_layer::Layer;
use tower_service::Service;

/// A browser family that [`AllowBrowser`] recognizes in the `User-Agent` header.
///
/// # Examples
///
/// ```
/// use ocre::security::Browser;
///
/// let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_4) AppleWebKit/605.1.15 (KHTML, like Gecko) \
///           Version/17.4 Safari/605.1.15";
/// assert_eq!(Browser::detect(ua), Some((Browser::Safari, (17, 4))));
/// assert_eq!(Browser::detect("curl/8.7.1"), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browser {
    /// Google Chrome and Chromium (`Chrome/`, `CriOS/` on iOS).
    Chrome,
    /// Microsoft Edge (`Edg/`, `EdgA/`, `EdgiOS/`).
    Edge,
    /// Mozilla Firefox (`Firefox/`).
    Firefox,
    /// Internet Explorer (`MSIE`, `Trident/`).
    InternetExplorer,
    /// Opera (`OPR/`).
    Opera,
    /// Apple Safari (`Version/... Safari/`).
    Safari,
}

impl Browser {
    /// The browser and its `(major, minor)` version named by a `User-Agent`, or `None` for anything else (bots, `curl`).
    ///
    /// Tokens are checked from the most specific: Edge and Opera also send
    /// `Chrome/`, and Chrome also sends `Safari/`. Pure CPU.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::Browser;
    ///
    /// let edge = "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 \
    ///             Safari/537.36 Edg/124.0.2478.51";
    /// assert_eq!(Browser::detect(edge), Some((Browser::Edge, (124, 0))));
    /// assert_eq!(Browser::detect("Mozilla/4.0 (compatible; MSIE 8.0; Windows NT 6.1)"),
    ///            Some((Browser::InternetExplorer, (8, 0))));
    /// ```
    pub fn detect(user_agent: &str) -> Option<(Browser, (u32, u32))> {
        const TOKENS: &[(Browser, &str)] = &[
            (Browser::InternetExplorer, "MSIE "),
            (Browser::InternetExplorer, "rv:"),
            (Browser::Edge, "Edg/"),
            (Browser::Edge, "EdgA/"),
            (Browser::Edge, "EdgiOS/"),
            (Browser::Opera, "OPR/"),
            (Browser::Firefox, "Firefox/"),
            (Browser::Chrome, "CriOS/"),
            (Browser::Chrome, "Chrome/"),
            (Browser::Safari, "Version/"),
        ];
        let trident = user_agent.contains("Trident/");
        TOKENS.iter().find_map(|&(browser, token)| {
            let applies = match (browser, token) {
                (Browser::InternetExplorer, "rv:") => trident,
                (Browser::Safari, _) => user_agent.contains("Safari/"),
                _ => true,
            };
            let start = user_agent.find(token).filter(|_| applies)? + token.len();
            Some((browser, version(&user_agent[start..])?))
        })
    }
}

/// `120.0.6099.71` is `(120, 0)`; `8.0;` is `(8, 0)`.
fn version(text: &str) -> Option<(u32, u32)> {
    let end = text.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(text.len());
    let mut parts = text[..end].split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|minor| minor.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// The page outdated browsers get, with status 406.
const UNSUPPORTED: &str = "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Your browser is not supported (406)</title>\
</head><body><h1>Your browser is not supported.</h1><p>Please upgrade your browser to continue.</p></body></html>\n";

/// Tower layer that answers `406 Not Acceptable` to browsers older than the versions it allows (Rails' `allow_browser`).
///
/// [`modern`](Self::modern) is Rails' `versions: :modern`: browsers with
/// WebP images, web push, badges, import maps, CSS nesting and CSS `:has`
/// (Safari 17.2, Chrome and Edge 120, Firefox 121, Opera 106; no Internet
/// Explorer). Requests whose `User-Agent` is missing or names no known
/// browser (bots, `curl`, API clients, uptime checks) always pass, as in
/// Rails. Outdated browsers get a short HTML page asking to upgrade
/// (replace it with [`page`](Self::page)). Pure CPU, no binding call.
///
/// Apply it to the routes of pages (`.layer(...)` on a `Router`), not to
/// JSON APIs or webhooks, whose clients are not browsers anyway.
///
/// # Examples
///
/// ```
/// use axum::{Router, routing::get};
/// use ocre::security::{AllowBrowser, Browser};
///
/// let app: Router = Router::new().route("/", get(|| async { "home" })).layer(AllowBrowser::modern());
/// # let _ = app;
///
/// let policy = AllowBrowser::new().minimum(Browser::Firefox, 115, 0).deny(Browser::InternetExplorer);
/// assert!(policy.allows(Some("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")));
/// assert!(!policy.allows(Some("Mozilla/5.0 (X11; Linux x86_64; rv:102.0) Gecko/20100101 Firefox/102.0")));
/// assert!(policy.allows(None));
/// ```
#[derive(Debug, Clone)]
pub struct AllowBrowser {
    rules: Vec<(Browser, Option<(u32, u32)>)>,
    page: Arc<str>,
}

impl Default for AllowBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl AllowBrowser {
    /// A policy that allows every browser; add limits with [`minimum`](Self::minimum) and [`deny`](Self::deny).
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(ocre::security::AllowBrowser::new().allows(Some("Mozilla/4.0 (compatible; MSIE 6.0)")));
    /// ```
    pub fn new() -> Self {
        Self { rules: Vec::new(), page: Arc::from(UNSUPPORTED) }
    }

    /// Rails' `allow_browser versions: :modern`: Safari 17.2, Chrome and Edge 120, Firefox 121, Opera 106, no Internet Explorer.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::AllowBrowser;
    ///
    /// let old_chrome = "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/109.0.0.0 Safari/537.36";
    /// assert!(!AllowBrowser::modern().allows(Some(old_chrome)));
    /// ```
    pub fn modern() -> Self {
        Self::new()
            .minimum(Browser::Safari, 17, 2)
            .minimum(Browser::Chrome, 120, 0)
            .minimum(Browser::Edge, 120, 0)
            .minimum(Browser::Firefox, 121, 0)
            .minimum(Browser::Opera, 106, 0)
            .deny(Browser::InternetExplorer)
    }

    /// Allows `browser` from version `major.minor` on; replaces an earlier rule for the same browser.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{AllowBrowser, Browser};
    ///
    /// let policy = AllowBrowser::modern().minimum(Browser::Safari, 16, 0);
    /// let safari_16 = "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 \
    ///                  (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1";
    /// assert!(policy.allows(Some(safari_16)));
    /// ```
    pub fn minimum(self, browser: Browser, major: u32, minor: u32) -> Self {
        self.rule(browser, Some((major, minor)))
    }

    /// Refuses every version of `browser` (Rails' `ie: false`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{AllowBrowser, Browser};
    ///
    /// let policy = AllowBrowser::new().deny(Browser::InternetExplorer);
    /// assert!(!policy.allows(Some("Mozilla/5.0 (Windows NT 10.0; Trident/7.0; rv:11.0) like Gecko")));
    /// ```
    pub fn deny(self, browser: Browser) -> Self {
        self.rule(browser, None)
    }

    fn rule(mut self, browser: Browser, minimum: Option<(u32, u32)>) -> Self {
        self.rules.retain(|(existing, _)| *existing != browser);
        self.rules.push((browser, minimum));
        self
    }

    /// Replaces the HTML page sent with the 406 (by default a short "please upgrade your browser" page).
    ///
    /// # Examples
    ///
    /// ```
    /// let policy = ocre::security::AllowBrowser::modern().page("<h1>Please use a recent browser.</h1>");
    /// # let _ = policy;
    /// ```
    pub fn page(mut self, html: impl Into<String>) -> Self {
        self.page = Arc::from(html.into());
        self
    }

    /// Whether a request with this `User-Agent` passes: a missing header or an unknown client always does.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::AllowBrowser;
    ///
    /// let policy = AllowBrowser::modern();
    /// assert!(policy.allows(Some("Googlebot/2.1 (+http://www.google.com/bot.html)")));
    /// assert!(policy.allows(None));
    /// ```
    pub fn allows(&self, user_agent: Option<&str>) -> bool {
        let Some((browser, version)) = user_agent.and_then(Browser::detect) else {
            return true;
        };
        match self.rules.iter().find(|(rule, _)| *rule == browser) {
            Some((_, Some(minimum))) => version >= *minimum,
            Some((_, None)) => false,
            None => true,
        }
    }

    fn refusal(&self) -> Response {
        let mut response = (StatusCode::NOT_ACCEPTABLE, self.page.to_string()).into_response();
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
        response
    }
}

impl<S> Layer<S> for AllowBrowser {
    type Service = AllowBrowserService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AllowBrowserService { inner, policy: Arc::new(self.clone()) }
    }
}

/// The service built by the [`AllowBrowser`] layer.
///
/// # Examples
///
/// ```
/// use axum::{Router, routing::get};
/// use ocre::security::AllowBrowser;
///
/// // Built by `.layer(AllowBrowser::modern())`; never named in app code.
/// let app: Router = Router::new().route("/", get(|| async { "home" })).layer(AllowBrowser::modern());
/// # let _ = app;
/// ```
#[derive(Debug, Clone)]
pub struct AllowBrowserService<S> {
    inner: S,
    policy: Arc<AllowBrowser>,
}

impl<S> Service<Request> for AllowBrowserService<S>
where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let user_agent = req.headers().get(header::USER_AGENT).and_then(|value| value.to_str().ok());
        if !self.policy.allows(user_agent) {
            let refusal = self.policy.refusal();
            return Box::pin(async move { Ok(refusal) });
        }
        Box::pin(self.inner.call(req))
    }
}

#[cfg(test)]
#[path = "../../tests/security/browser.rs"]
mod tests;
