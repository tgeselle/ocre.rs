use askama::Template;
use axum::{Router, response::Html, routing::get};
use ocre::{Ctx, Result, render};
use worker::{Context, Env, HttpRequest, event};

// ocre:modules

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> worker::Result<worker::web_sys::Response> {
    ocre::serve(routes(), req, env).await
}

fn routes() -> Router<Ctx> {
    Router::new()
        .route("/", get(home))
        .route("/up", get(up))
        // ocre:routes
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
