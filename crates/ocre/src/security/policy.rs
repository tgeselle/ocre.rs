//! `Content-Security-Policy` and `Permissions-Policy` headers, as tower layers.

use std::{
    convert::Infallible,
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    extract::{FromRequestParts, Request},
    http::{HeaderName, HeaderValue, header, request::Parts},
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use tower_layer::Layer;
use tower_service::Service;

use crate::{
    Error,
    session::{Rejection, reject},
};

/// `'self'`: the app's own origin, in a [`ContentSecurityPolicy`] or [`PermissionsPolicy`] source list.
///
/// # Examples
///
/// ```
/// use ocre::security::{ContentSecurityPolicy, SELF};
///
/// assert_eq!(ContentSecurityPolicy::new().default_src(&[SELF]).header_value(None), "default-src 'self'");
/// ```
pub const SELF: &str = "'self'";
/// `'none'`: nothing is allowed, in a [`ContentSecurityPolicy`] source list.
///
/// # Examples
///
/// ```
/// use ocre::security::{ContentSecurityPolicy, NONE};
///
/// assert_eq!(ContentSecurityPolicy::new().object_src(&[NONE]).header_value(None), "object-src 'none'");
/// ```
pub const NONE: &str = "'none'";
/// Placeholder for the request's nonce: sent as `'nonce-<value>'` (Rails' `content_security_policy_nonce`).
///
/// Put it in `script_src` or `style_src`; the layer generates a new random
/// nonce per request and handlers get it with the [`CspNonce`] extractor, to
/// write `<script nonce="{{ nonce }}">`.
///
/// # Examples
///
/// ```
/// use ocre::security::{ContentSecurityPolicy, NONCE, SELF};
///
/// let csp = ContentSecurityPolicy::new().script_src(&[SELF, NONCE]);
/// assert_eq!(csp.header_value(Some("abc")), "script-src 'self' 'nonce-abc'");
/// ```
pub const NONCE: &str = "'nonce'";
/// `'unsafe-inline'`: allows inline `<style>`/`<script>` and `style=` attributes. Avoid it for scripts.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::security::UNSAFE_INLINE, "'unsafe-inline'");
/// ```
pub const UNSAFE_INLINE: &str = "'unsafe-inline'";
/// `'unsafe-eval'`: allows `eval` and `new Function` (htmx's `hx-on` and `js:` need it). Avoid it.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::security::UNSAFE_EVAL, "'unsafe-eval'");
/// ```
pub const UNSAFE_EVAL: &str = "'unsafe-eval'";
/// `'strict-dynamic'`: scripts loaded by a nonced script are trusted too.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::security::STRICT_DYNAMIC, "'strict-dynamic'");
/// ```
pub const STRICT_DYNAMIC: &str = "'strict-dynamic'";
/// `data:` URLs (inline images, fonts).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::security::DATA, "data:");
/// ```
pub const DATA: &str = "data:";
/// `blob:` URLs (files built in the browser).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::security::BLOB, "blob:");
/// ```
pub const BLOB: &str = "blob:";
/// Any `https:` URL.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::security::HTTPS, "https:");
/// ```
pub const HTTPS: &str = "https:";

/// A `Content-Security-Policy` header, built directive by directive, and the layer that sends it.
///
/// Rails' `content_security_policy` initializer as plain Rust: build the
/// policy in `src/lib.rs` and add it to the router with `.layer(policy)`
/// (generated apps do, in `content_security_policy()`). Every response gets
/// the header unless the handler (or a layer closer to it) set one already,
/// which is how routes override the global policy: give a nested router its
/// own `.layer(...)`, or return the header from the handler.
///
/// Sources are written as in the header: [`SELF`], [`NONE`], [`DATA`],
/// `"https://unpkg.com"`, ... [`NONCE`] stands for the request's random nonce
/// (see [`CspNonce`]). Calling a directive twice replaces it.
/// [`report_only`](Self::report_only) sends
/// `Content-Security-Policy-Report-Only` instead, to try a policy without
/// breaking pages; [`report_uri`](Self::report_uri) and
/// [`report_to`](Self::report_to) collect violations.
///
/// # Free plan
///
/// A header per response and, when the policy uses [`NONCE`], 16 random
/// bytes per request: no binding call, microseconds of CPU.
///
/// # Examples
///
/// ```
/// use axum::{Router, routing::get};
/// use ocre::security::{ContentSecurityPolicy, DATA, NONCE, NONE, SELF};
///
/// let policy = ContentSecurityPolicy::new()
///     .default_src(&[SELF])
///     .script_src(&[SELF, NONCE, "https://unpkg.com"])
///     .img_src(&[SELF, DATA])
///     .object_src(&[NONE])
///     .report_uri("/csp-reports");
/// assert_eq!(
///     policy.header_value(Some("r4nd0m")),
///     "default-src 'self'; script-src 'self' 'nonce-r4nd0m' https://unpkg.com; img-src 'self' data:; \
///      object-src 'none'; report-uri /csp-reports"
/// );
///
/// let app: Router = Router::new().route("/", get(|| async { "home" })).layer(policy);
/// # let _ = app;
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentSecurityPolicy {
    directives: Vec<(String, Vec<String>)>,
    report_only: bool,
}

impl ContentSecurityPolicy {
    /// An empty policy: add directives with the builder methods.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::security::ContentSecurityPolicy::new().header_value(None), "");
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets any directive, e.g. `directive("sandbox", &["allow-forms"])`; replaces a previous value.
    ///
    /// An empty `sources` list writes the directive alone
    /// (`upgrade-insecure-requests`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// let csp = ContentSecurityPolicy::new().directive("worker-src", &[SELF]).directive("worker-src", &["blob:"]);
    /// assert_eq!(csp.header_value(None), "worker-src blob:");
    /// ```
    #[must_use]
    pub fn directive(mut self, name: &str, sources: &[&str]) -> Self {
        let sources = sources.iter().map(|source| (*source).to_owned()).collect();
        match self.directives.iter_mut().find(|(existing, _)| existing == name) {
            Some((_, existing)) => *existing = sources,
            None => self.directives.push((name.to_owned(), sources)),
        }
        self
    }

    /// `default-src`: the fallback for every fetch directive not set.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// assert_eq!(ContentSecurityPolicy::new().default_src(&[SELF]).header_value(None), "default-src 'self'");
    /// ```
    #[must_use]
    pub fn default_src(self, sources: &[&str]) -> Self {
        self.directive("default-src", sources)
    }

    /// `script-src`: where scripts may come from.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// let csp = ContentSecurityPolicy::new().script_src(&[SELF, "https://unpkg.com"]);
    /// assert_eq!(csp.header_value(None), "script-src 'self' https://unpkg.com");
    /// ```
    #[must_use]
    pub fn script_src(self, sources: &[&str]) -> Self {
        self.directive("script-src", sources)
    }

    /// `style-src`: where stylesheets and inline styles may come from.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF, UNSAFE_INLINE};
    ///
    /// let csp = ContentSecurityPolicy::new().style_src(&[SELF, UNSAFE_INLINE]);
    /// assert_eq!(csp.header_value(None), "style-src 'self' 'unsafe-inline'");
    /// ```
    #[must_use]
    pub fn style_src(self, sources: &[&str]) -> Self {
        self.directive("style-src", sources)
    }

    /// `img-src`: images and favicons.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, DATA, SELF};
    ///
    /// assert_eq!(ContentSecurityPolicy::new().img_src(&[SELF, DATA]).header_value(None), "img-src 'self' data:");
    /// ```
    #[must_use]
    pub fn img_src(self, sources: &[&str]) -> Self {
        self.directive("img-src", sources)
    }

    /// `font-src`: web fonts.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// assert_eq!(ContentSecurityPolicy::new().font_src(&[SELF]).header_value(None), "font-src 'self'");
    /// ```
    #[must_use]
    pub fn font_src(self, sources: &[&str]) -> Self {
        self.directive("font-src", sources)
    }

    /// `connect-src`: `fetch`, XHR (htmx requests), WebSockets and `EventSource`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// assert_eq!(ContentSecurityPolicy::new().connect_src(&[SELF]).header_value(None), "connect-src 'self'");
    /// ```
    #[must_use]
    pub fn connect_src(self, sources: &[&str]) -> Self {
        self.directive("connect-src", sources)
    }

    /// `media-src`: `<audio>` and `<video>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// assert_eq!(ContentSecurityPolicy::new().media_src(&[SELF]).header_value(None), "media-src 'self'");
    /// ```
    #[must_use]
    pub fn media_src(self, sources: &[&str]) -> Self {
        self.directive("media-src", sources)
    }

    /// `object-src`: `<object>` and `<embed>`; set it to [`NONE`].
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, NONE};
    ///
    /// assert_eq!(ContentSecurityPolicy::new().object_src(&[NONE]).header_value(None), "object-src 'none'");
    /// ```
    #[must_use]
    pub fn object_src(self, sources: &[&str]) -> Self {
        self.directive("object-src", sources)
    }

    /// `frame-src`: pages this app may put in an `<iframe>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::ContentSecurityPolicy;
    ///
    /// let csp = ContentSecurityPolicy::new().frame_src(&["https://www.youtube-nocookie.com"]);
    /// assert_eq!(csp.header_value(None), "frame-src https://www.youtube-nocookie.com");
    /// ```
    #[must_use]
    pub fn frame_src(self, sources: &[&str]) -> Self {
        self.directive("frame-src", sources)
    }

    /// `frame-ancestors`: sites that may put this app in a frame (the modern `X-Frame-Options`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// assert_eq!(ContentSecurityPolicy::new().frame_ancestors(&[SELF]).header_value(None), "frame-ancestors 'self'");
    /// ```
    #[must_use]
    pub fn frame_ancestors(self, sources: &[&str]) -> Self {
        self.directive("frame-ancestors", sources)
    }

    /// `form-action`: where forms may be submitted.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// assert_eq!(ContentSecurityPolicy::new().form_action(&[SELF]).header_value(None), "form-action 'self'");
    /// ```
    #[must_use]
    pub fn form_action(self, sources: &[&str]) -> Self {
        self.directive("form-action", sources)
    }

    /// `base-uri`: allowed `<base href>` values.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// assert_eq!(ContentSecurityPolicy::new().base_uri(&[SELF]).header_value(None), "base-uri 'self'");
    /// ```
    #[must_use]
    pub fn base_uri(self, sources: &[&str]) -> Self {
        self.directive("base-uri", sources)
    }

    /// `upgrade-insecure-requests`: browsers load `http:` resources over HTTPS.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::ContentSecurityPolicy;
    ///
    /// let csp = ContentSecurityPolicy::new().upgrade_insecure_requests();
    /// assert_eq!(csp.header_value(None), "upgrade-insecure-requests");
    /// ```
    #[must_use]
    pub fn upgrade_insecure_requests(self) -> Self {
        self.directive("upgrade-insecure-requests", &[])
    }

    /// `report-uri`: where browsers POST violation reports (JSON), e.g. a route of the app.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::ContentSecurityPolicy;
    ///
    /// let csp = ContentSecurityPolicy::new().report_uri("/csp-reports");
    /// assert_eq!(csp.header_value(None), "report-uri /csp-reports");
    /// ```
    #[must_use]
    pub fn report_uri(self, uri: &str) -> Self {
        self.directive("report-uri", &[uri])
    }

    /// `report-to`: the `Reporting-Endpoints` group violation reports go to.
    ///
    /// Send the `Reporting-Endpoints: csp="/csp-reports"` header too
    /// (browsers without Reporting API support use [`report_uri`](Self::report_uri)).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::ContentSecurityPolicy;
    ///
    /// assert_eq!(ContentSecurityPolicy::new().report_to("csp").header_value(None), "report-to csp");
    /// ```
    #[must_use]
    pub fn report_to(self, group: &str) -> Self {
        self.directive("report-to", &[group])
    }

    /// Sends `Content-Security-Policy-Report-Only`: browsers report violations but block nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, SELF};
    ///
    /// let csp = ContentSecurityPolicy::new().default_src(&[SELF]).report_only();
    /// assert_eq!(csp.header_name(), "content-security-policy-report-only");
    /// ```
    #[must_use]
    pub fn report_only(mut self) -> Self {
        self.report_only = true;
        self
    }

    /// `content-security-policy`, or `content-security-policy-report-only` after
    /// [`report_only`](Self::report_only).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::security::ContentSecurityPolicy::new().header_name(), "content-security-policy");
    /// ```
    pub fn header_name(&self) -> HeaderName {
        if self.report_only { header::CONTENT_SECURITY_POLICY_REPORT_ONLY } else { header::CONTENT_SECURITY_POLICY }
    }

    /// The header value, with [`NONCE`] replaced by `'nonce-<nonce>'` (and dropped when `nonce` is `None`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{ContentSecurityPolicy, NONCE, SELF};
    ///
    /// let csp = ContentSecurityPolicy::new().default_src(&[SELF]).script_src(&[SELF, NONCE]);
    /// assert_eq!(csp.header_value(Some("n0nce")), "default-src 'self'; script-src 'self' 'nonce-n0nce'");
    /// assert_eq!(csp.header_value(None), "default-src 'self'; script-src 'self'");
    /// ```
    pub fn header_value(&self, nonce: Option<&str>) -> String {
        let mut value = String::new();
        for (name, sources) in &self.directives {
            if !value.is_empty() {
                value.push_str("; ");
            }
            value.push_str(name);
            for source in sources {
                if source == NONCE {
                    let Some(nonce) = nonce else { continue };
                    value.push_str(" 'nonce-");
                    value.push_str(nonce);
                    value.push('\'');
                } else {
                    value.push(' ');
                    value.push_str(source);
                }
            }
        }
        value
    }

    fn uses_nonce(&self) -> bool {
        self.directives.iter().any(|(_, sources)| sources.iter().any(|source| source == NONCE))
    }
}

/// The request's Content-Security-Policy nonce, as an extractor (Rails' `content_security_policy_nonce`).
///
/// A new random value (16 bytes, base64) per request, created by the
/// [`ContentSecurityPolicy`] layer and sent as `'nonce-<value>'` wherever the
/// policy lists [`NONCE`]. Pass it to the template and write
/// `<script nonce="{{ nonce }}">`; `<meta name="csp-nonce" content="{{ nonce }}">`
/// exposes it to JavaScript (Rails' `csp_meta_tag`). Rejects with
/// [`Error::Internal`] when no policy layer wraps the route.
///
/// # Examples
///
/// ```no_run
/// use axum::response::Html;
/// use ocre::security::CspNonce;
///
/// async fn page(nonce: CspNonce) -> Html<String> {
///     Html(format!(r#"<script nonce="{nonce}">console.log("allowed")</script>"#))
/// }
/// # let _ = page;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CspNonce(String);

impl CspNonce {
    fn generate() -> Self {
        Self(STANDARD.encode(crate::token::random_bytes::<16>()))
    }

    /// The nonce value, without the `'nonce-...'` wrapper.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// async fn page(nonce: ocre::security::CspNonce) -> String {
    ///     nonce.as_str().to_owned()
    /// }
    /// # let _ = page;
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CspNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<S: Sync> FromRequestParts<S> for CspNonce {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Rejection> {
        parts.extensions.get::<CspNonce>().cloned().ok_or_else(|| {
            reject(Error::internal(
                "no CSP nonce on this request. Fix: add `.layer(content_security_policy())` (an \
                 `ocre::security::ContentSecurityPolicy`) to the router serving this route",
            ))
        })
    }
}

/// A `Permissions-Policy` header, built feature by feature, and the layer that sends it.
///
/// Rails' `permissions_policy` initializer: it turns browser features
/// (camera, microphone, geolocation, ...) off for the app and for the frames
/// it embeds. Add it with `.layer(policy)`; like [`ContentSecurityPolicy`],
/// a handler's own header wins, so routes can override it.
///
/// In allowlists, [`SELF`] (or `"self"`) and `"*"` are keywords; other
/// entries are origins, quoted in the header.
///
/// # Free plan
///
/// One header per response; no binding call.
///
/// # Examples
///
/// ```
/// use axum::{Router, routing::get};
/// use ocre::security::{PermissionsPolicy, SELF};
///
/// let policy = PermissionsPolicy::new()
///     .deny(&["camera", "microphone", "geolocation"])
///     .allow("fullscreen", &[SELF, "https://player.example"]);
/// assert_eq!(
///     policy.header_value(),
///     r#"camera=(), microphone=(), geolocation=(), fullscreen=(self "https://player.example")"#
/// );
///
/// let app: Router = Router::new().route("/", get(|| async { "home" })).layer(policy);
/// # let _ = app;
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionsPolicy {
    features: Vec<(String, Vec<String>)>,
}

impl PermissionsPolicy {
    /// An empty policy: add features with [`allow`](Self::allow) and [`deny`](Self::deny).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::security::PermissionsPolicy::new().header_value(), "");
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Allows `feature` for the listed origins only; replaces a previous rule for it.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::{PermissionsPolicy, SELF};
    ///
    /// assert_eq!(PermissionsPolicy::new().allow("geolocation", &[SELF]).header_value(), "geolocation=(self)");
    /// assert_eq!(PermissionsPolicy::new().allow("autoplay", &["*"]).header_value(), "autoplay=*");
    /// ```
    #[must_use]
    pub fn allow(mut self, feature: &str, allowlist: &[&str]) -> Self {
        let allowlist = allowlist
            .iter()
            .map(|origin| match *origin {
                SELF | "self" => "self".to_owned(),
                "*" => "*".to_owned(),
                origin => format!("\"{}\"", origin.replace(['"', '\\'], "")),
            })
            .collect();
        match self.features.iter_mut().find(|(existing, _)| existing == feature) {
            Some((_, existing)) => *existing = allowlist,
            None => self.features.push((feature.to_owned(), allowlist)),
        }
        self
    }

    /// Turns `features` off everywhere: `camera=()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::security::PermissionsPolicy;
    ///
    /// assert_eq!(PermissionsPolicy::new().deny(&["camera", "usb"]).header_value(), "camera=(), usb=()");
    /// ```
    #[must_use]
    pub fn deny(self, features: &[&str]) -> Self {
        features.iter().fold(self, |policy, feature| policy.allow(feature, &[]))
    }

    /// The header value.
    ///
    /// # Examples
    ///
    /// ```
    /// let policy = ocre::security::PermissionsPolicy::new().deny(&["payment"]);
    /// assert_eq!(policy.header_value(), "payment=()");
    /// ```
    pub fn header_value(&self) -> String {
        let rules: Vec<String> = self
            .features
            .iter()
            .map(|(feature, allowlist)| match allowlist.as_slice() {
                [star] if star == "*" => format!("{feature}=*"),
                _ => format!("{feature}=({})", allowlist.join(" ")),
            })
            .collect();
        rules.join(", ")
    }
}

mod sealed {
    use axum::{
        extract::Request,
        http::{HeaderName, HeaderValue},
    };

    /// What a policy layer adds to a request and its response. Sealed:
    /// implemented by [`ContentSecurityPolicy`](super::ContentSecurityPolicy)
    /// and [`PermissionsPolicy`](super::PermissionsPolicy) only.
    pub trait Policy: Send + Sync + 'static {
        /// The header name and value for this request; may add extensions (the nonce).
        fn prepare(&self, req: &mut Request) -> (HeaderName, HeaderValue);
        /// Headers that, when the response already has one, mean the handler chose its own policy.
        fn overridden_by(&self) -> [HeaderName; 2];
    }
}

use sealed::Policy;

impl Policy for ContentSecurityPolicy {
    fn prepare(&self, req: &mut Request) -> (HeaderName, HeaderValue) {
        let nonce = match req.extensions().get::<CspNonce>() {
            Some(nonce) => nonce.clone(),
            None => CspNonce::generate(),
        };
        let value = self.header_value(self.uses_nonce().then_some(nonce.as_str()));
        req.extensions_mut().insert(nonce);
        (self.header_name(), header_value(value))
    }

    fn overridden_by(&self) -> [HeaderName; 2] {
        [header::CONTENT_SECURITY_POLICY, header::CONTENT_SECURITY_POLICY_REPORT_ONLY]
    }
}

impl Policy for PermissionsPolicy {
    fn prepare(&self, _req: &mut Request) -> (HeaderName, HeaderValue) {
        (permissions_policy(), header_value(self.header_value()))
    }

    fn overridden_by(&self) -> [HeaderName; 2] {
        [permissions_policy(), permissions_policy()]
    }
}

fn permissions_policy() -> HeaderName {
    HeaderName::from_static("permissions-policy")
}

/// Sources come from app code; characters a header cannot carry are dropped.
fn header_value(value: String) -> HeaderValue {
    HeaderValue::try_from(value).unwrap_or_else(|err| {
        crate::error::log_internal(&format!("invalid security policy header, not sent: {err}"));
        HeaderValue::from_static("")
    })
}

impl<S> Layer<S> for ContentSecurityPolicy {
    type Service = PolicyService<S, ContentSecurityPolicy>;

    fn layer(&self, inner: S) -> Self::Service {
        PolicyService { inner, policy: Arc::new(self.clone()) }
    }
}

impl<S> Layer<S> for PermissionsPolicy {
    type Service = PolicyService<S, PermissionsPolicy>;

    fn layer(&self, inner: S) -> Self::Service {
        PolicyService { inner, policy: Arc::new(self.clone()) }
    }
}

/// The service a [`ContentSecurityPolicy`] or [`PermissionsPolicy`] layer wraps routes in.
///
/// Not used directly: `router.layer(policy)` builds it.
///
/// # Examples
///
/// ```
/// use axum::{Router, routing::get};
/// use ocre::security::{PermissionsPolicy, PolicyService};
/// use tower_layer::Layer as _;
///
/// let router: Router = Router::new().route("/", get(|| async { "home" }));
/// let service: PolicyService<Router, PermissionsPolicy> = PermissionsPolicy::new().layer(router);
/// # let _ = service;
/// ```
pub struct PolicyService<S, P> {
    inner: S,
    policy: Arc<P>,
}

impl<S: Clone, P> Clone for PolicyService<S, P> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), policy: Arc::clone(&self.policy) }
    }
}

impl<S, P> Service<Request> for PolicyService<S, P>
where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
    P: Policy,
{
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        let (name, value) = self.policy.prepare(&mut req);
        let overridden_by = self.policy.overridden_by();
        // The service polled ready is the one that must be called.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let mut response = inner.call(req).await?;
            let headers = response.headers_mut();
            if !overridden_by.iter().any(|name| headers.contains_key(name)) && !value.is_empty() {
                headers.insert(name, value);
            }
            Ok(response)
        })
    }
}

#[cfg(test)]
#[path = "../../tests/security/policy.rs"]
mod tests;
