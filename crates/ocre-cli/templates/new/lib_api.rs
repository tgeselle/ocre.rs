use axum::{Router, routing::get};
use ocre::{Ctx, Json};
use serde::Serialize;
use worker::{Context, Env, HttpRequest, event};

// ocre:modules

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> worker::Result<axum::http::Response<axum::body::Body>> {
    ocre::serve(routes(), req, env).await
}

fn routes() -> Router<Ctx> {
    Router::new()
        .route("/", get(status))
        .route("/up", get(up))
        // ocre:routes
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
