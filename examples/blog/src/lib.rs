use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use ocre::{Ctx, Error, Htmx, OptionExt, Result, params, render};
use serde::Deserialize;
use worker::{Context, Env, HttpRequest, event};

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> worker::Result<axum::http::Response<axum::body::Body>> {
    ocre::serve(routes(), req, env).await
}

fn routes() -> Router<Ctx> {
    Router::new()
        .route("/", get(index))
        .route("/posts", post(create))
        .route("/posts/{id}", get(show))
}

#[derive(Deserialize)]
struct Post {
    id: i64,
    title: String,
    body: String,
    created_at: String,
}

#[derive(Deserialize)]
struct NewPost {
    title: String,
    body: String,
}

#[derive(Template)]
#[template(path = "posts/index.html")]
struct IndexView {
    posts: Vec<Post>,
}

#[derive(Template)]
#[template(path = "posts/show.html")]
struct ShowView {
    post: Post,
}

#[derive(Template)]
#[template(path = "posts/_post.html")]
struct PostFragment {
    post: Post,
}

async fn index(State(ctx): State<Ctx>) -> Result<Html<String>> {
    let posts = ctx.db()?.all("SELECT * FROM posts ORDER BY id DESC", params![]).await?;
    render(&IndexView { posts })
}

async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Html<String>> {
    let post = ctx.db()?.first("SELECT * FROM posts WHERE id = ?1", params![id]).await?.or_404()?;
    render(&ShowView { post })
}

async fn create(State(ctx): State<Ctx>, Htmx(htmx): Htmx, Form(input): Form<NewPost>) -> Result<Response> {
    let title = input.title.trim();
    let body = input.body.trim();
    if title.is_empty() || body.is_empty() {
        return Err(Error::bad_request("Title and body are required."));
    }
    let post: Post = ctx
        .db()?
        .first("INSERT INTO posts (title, body) VALUES (?1, ?2) RETURNING *", params![title, body])
        .await?
        .ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))?;
    if htmx {
        Ok(render(&PostFragment { post })?.into_response())
    } else {
        Ok(Redirect::to(&format!("/posts/{}", post.id)).into_response())
    }
}
