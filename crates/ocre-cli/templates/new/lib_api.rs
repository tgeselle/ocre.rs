use axum::{Router, routing::get};
use ocre::{ApiError, Ctx, Error, Json};
use serde::Serialize;
use worker::{Context, Env, HttpRequest, event};

// ocre:modules

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> worker::Result<worker::web_sys::Response> {
    ocre::serve(routes(), req, env).await
}

fn routes() -> Router<Ctx> {
    Router::new()
        .route("/", get(status))
        .route("/up", get(up))
        // ocre:routes
        .fallback(not_found)
}

#[derive(Serialize)]
struct Status {
    app: &'static str,
    status: &'static str,
}

async fn status() -> Json<Status> {
    Json(Status { app: "__APP_NAME__", status: "ok" })
}

/// Health check for uptime monitors and load balancers, like Rails' `/up`:
/// 200 `OK` whenever the Worker runs.
async fn up() -> &'static str {
    "OK"
}

/// Paths no route matches: `{"error": {"status": 404, "message": "Not found"}}`.
async fn not_found() -> ApiError {
    ApiError(Error::NotFound)
}
