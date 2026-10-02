//! Questions in an event's room: the audience asks and votes, the host marks
//! questions answered or deletes them. After each change the event's list is
//! rendered again and broadcast (channels in src/realtime.rs), so it reorders
//! live in every open room.

use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, State},
    http::{StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::post,
};
use ocre::{Ctx, Error, Flash, Htmx, OptionExt, Result, Session, realtime, render};
use serde::Deserialize;

use crate::auth::{CurrentUser, OptionalUser};
use crate::events;
use crate::models::event::{self, Event};
use crate::models::question::{self, NewQuestion, Question, QuestionChanges};
use crate::models::user::User;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/events/{id}/questions", post(ask))
        .route("/questions/{id}/vote", post(vote))
        .route("/questions/{id}/answer", post(answer))
        .route("/questions/{id}/delete", post(delete))
}

pub mod paths {
    use std::fmt::Display;

    /// `id` is the event's public id.
    pub fn ask(id: impl Display) -> String {
        format!("/events/{id}/questions")
    }

    pub fn vote(id: impl Display) -> String {
        format!("/questions/{id}/vote")
    }

    pub fn answer(id: impl Display) -> String {
        format!("/questions/{id}/answer")
    }

    pub fn delete(id: impl Display) -> String {
        format!("/questions/{id}/delete")
    }
}

/// What the ask form submits.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct QuestionForm {
    pub body: String,
}

/// The question list of a room, also sent to open rooms on every change.
/// `host` adds the moderation buttons.
#[derive(Template)]
#[template(path = "questions/_list.html")]
struct ListView<'a> {
    questions: &'a [Question],
    host: bool,
}

/// The channel every visitor of the room listens to.
pub fn channel(event: &Event) -> String {
    format!("event:{}", event.public_id)
}

/// The host's channel: the same list, with moderation buttons.
pub fn host_channel(event: &Event) -> String {
    format!("host:{}", event.public_id)
}

/// Sends the event's current list to both channels. Best effort: a failed
/// broadcast is logged by Ocre and never fails the request.
async fn broadcast(ctx: &Ctx, event: &Event) -> Result<()> {
    let questions = question::for_event(ctx, event.id).await?;
    let audience = render(&ListView { questions: &questions, host: false })?.0;
    let host = render(&ListView { questions: &questions, host: true })?.0;
    realtime::broadcast(ctx, &channel(event), &realtime::update("questions", &audience)).await.ok();
    realtime::broadcast(ctx, &host_channel(event), &realtime::update("questions", &host)).await.ok();
    Ok(())
}

async fn ask(
    State(ctx): State<Ctx>,
    session: Session,
    flash: Flash,
    OptionalUser(user): OptionalUser,
    uri: Uri,
    Path(key): Path<String>,
    Form(form): Form<QuestionForm>,
) -> Result<Response> {
    let event = event::find_by_public_id(&ctx, &key).await?.or_404()?;
    let new = NewQuestion { event_id: event.id, body: form.body.trim().to_owned(), votes: 0, answered: false };
    match question::create(&ctx, new).await {
        Ok(_) => {
            broadcast(&ctx, &event).await?;
            session.flash("notice", "Your question is in.")?;
            Ok(Redirect::to(&events::paths::show(&key)).into_response())
        }
        Err(Error::Invalid(errors)) => {
            let room = events::room(&ctx, flash, &uri, event, user, form, errors).await?;
            Ok((StatusCode::UNPROCESSABLE_ENTITY, room).into_response())
        }
        Err(err) => Err(err),
    }
}

/// Session key: the questions this browser voted for.
const VOTED: &str = "voted";

/// One vote per question and browser; voting again changes nothing.
async fn vote(State(ctx): State<Ctx>, session: Session, Htmx(is_htmx): Htmx, Path(id): Path<i64>) -> Result<Response> {
    let record = question::find(&ctx, id).await?.or_404()?;
    let event = record.event(&ctx).await?.or_404()?;
    let mut voted: Vec<i64> = session.get(VOTED)?.unwrap_or_default();
    if !voted.contains(&id) {
        question::vote(&ctx, id).await?;
        // The session is a cookie (4 KB at most): remember the last 200 votes.
        if voted.len() >= 200 {
            voted.remove(0);
        }
        voted.push(id);
        session.insert(VOTED, &voted)?;
        broadcast(&ctx, &event).await?;
    }
    Ok(done(is_htmx, &event))
}

/// Marks a question answered, or open again.
async fn answer(
    State(ctx): State<Ctx>,
    CurrentUser(user): CurrentUser,
    Htmx(is_htmx): Htmx,
    Path(id): Path<i64>,
) -> Result<Response> {
    let (record, event) = hosted(&ctx, &user, id).await?;
    let changes = QuestionChanges { answered: Some(!record.answered), ..Default::default() };
    question::update(&ctx, id, changes).await?;
    broadcast(&ctx, &event).await?;
    Ok(done(is_htmx, &event))
}

async fn delete(
    State(ctx): State<Ctx>,
    CurrentUser(user): CurrentUser,
    Htmx(is_htmx): Htmx,
    Path(id): Path<i64>,
) -> Result<Response> {
    let (_, event) = hosted(&ctx, &user, id).await?;
    question::delete(&ctx, id).await?;
    broadcast(&ctx, &event).await?;
    Ok(done(is_htmx, &event))
}

/// The question `id` and its event, when `user` hosts that event.
async fn hosted(ctx: &Ctx, user: &User, id: i64) -> Result<(Question, Event)> {
    let record = question::find(ctx, id).await?.or_404()?;
    let event = record.event(ctx).await?.or_404()?;
    if event.user_id != user.id {
        return Err(Error::Forbidden);
    }
    Ok((record, event))
}

/// htmx buttons get a 204 (the broadcast updates the page); a plain form
/// post goes back to the room.
fn done(is_htmx: bool, event: &Event) -> Response {
    if is_htmx {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Redirect::to(&events::paths::show(&event.public_id)).into_response()
    }
}
