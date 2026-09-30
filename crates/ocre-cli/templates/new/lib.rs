use askama::Template;
use axum::{
    Router,
    middleware::map_response,
    response::{Html, Response},
    routing::get,
};
use ocre::{
    Ctx, Error, ErrorPage, Result, render,
    security::{ContentSecurityPolicy, DATA, HTTPS, NONE, PermissionsPolicy, SELF, UNSAFE_INLINE},
};
use worker::{Context, Env, HttpRequest, event};

// ocre:modules

/// Runs once when a Worker instance starts, before its first request, job or
/// cron run: the app's initializers. Register error reporters here, e.g.
/// `ocre::errors::subscribe(ocre::errors::Sentry);` (needs the SENTRY_DSN secret).
/// Keep it cheap: its CPU time counts against the first request.
#[event(start)]
fn start() {}

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> worker::Result<worker::web_sys::Response> {
    ocre::serve(routes(), req, env).await
}

fn routes() -> Router<Ctx> {
    Router::new()
        .route("/", get(home))
        .route("/up", get(up))
        // ocre:routes
        .fallback(not_found)
        .layer(map_response(error_page))
        .layer(content_security_policy())
        .layer(permissions_policy())
}

/// The Content-Security-Policy of every response (Rails' content_security_policy
/// initializer): scripts only from this app and unpkg.com (htmx), no inline
/// scripts or `onclick=` handlers, which blocks most XSS. For an inline script,
/// add `NONCE` to `script_src`, take `nonce: ocre::security::CspNonce` in the
/// handler and write `<script nonce="{{ nonce }}">`. A route can send its own
/// policy: a handler's header (or a nested router's `.layer(...)`) wins.
fn content_security_policy() -> ContentSecurityPolicy {
    ContentSecurityPolicy::new()
        .default_src(&[SELF])
        .script_src(&[SELF, "https://unpkg.com"])
        // The layout's <style> and htmx's indicator styles are inline.
        .style_src(&[SELF, UNSAFE_INLINE])
        .img_src(&[SELF, DATA, HTTPS])
        .font_src(&[SELF, DATA])
        .object_src(&[NONE])
        .base_uri(&[SELF])
        .frame_ancestors(&[SELF])
}

/// Browser features the app does not use are turned off (Rails' permissions_policy).
fn permissions_policy() -> PermissionsPolicy {
    PermissionsPolicy::new().deny(&["camera", "microphone", "geolocation", "payment", "usb"])
}

#[derive(Template)]
#[template(path = "home.html")]
struct HomeView;

async fn home() -> Result<Html<String>> {
    render(&HomeView)
}

/// Health check for uptime monitors and load balancers, like Rails' `/up`:
/// 200 `OK` whenever the Worker runs.
async fn up() -> &'static str {
    "OK"
}

/// Paths no route matches: the 404 error page.
async fn not_found() -> Error {
    Error::NotFound
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorView<'a> {
    error: &'a ErrorPage,
}

/// Renders error responses (404, 422, 500...) with templates/error.html,
/// like Rails' public/404.html. JSON errors and pages that set their own
/// status are left alone.
async fn error_page(response: Response) -> Response {
    ocre::error_page(response, |error| render(&ErrorView { error }))
}
