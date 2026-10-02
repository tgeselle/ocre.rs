//! Request tests for the questions of a room: asking, voting, moderating and
//! the live updates. They run against the app in workerd with
//! `ocre test --e2e`; plain `cargo test` skips them (`#[ignore]`).

use ocre::testing::{Client, sequence, sql};

/// A signed-up host and the public id of their new event.
fn host_with_event() -> (Client, String) {
    let mut client = Client::new();
    // Unique across test files, which share the test database.
    let email = format!("host{}-{}@example.com", std::process::id(), sequence());
    client.post("/signup", &[("email", email.as_str()), ("password", "correct horse")]).assert_status(303);
    let created = client.post("/events", &[("name", "Q&A")]);
    let location = created.assert_status(303).location().unwrap_or_default().to_owned();
    (client, location.trim_start_matches("/events/").to_owned())
}

/// `client` asks `body` in the room `event`; returns the question's id.
fn ask(client: &mut Client, event: &str, body: &str) -> i64 {
    client
        .post(&format!("/events/{event}/questions"), &[("body", body)])
        .assert_redirect_to(&format!("/events/{event}"));
    let rows = sql(&format!("SELECT id FROM questions WHERE body = {}", ocre::testing::quote(body)));
    rows[0]["id"].as_i64().expect("the question is saved")
}

fn votes(id: i64) -> i64 {
    sql(&format!("SELECT votes FROM questions WHERE id = {id}"))[0]["votes"].as_i64().unwrap_or_default()
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn a_question_appears_live_in_every_room() {
    let (_, event) = host_with_event();
    let mut visitor = Client::new();
    ask(&mut visitor, &event, "Will there be pizza?");
    visitor.get(&format!("/events/{event}")).assert_contains("Will there be pizza?");
    let sent = visitor.broadcasts();
    let audience = sent.iter().rev().find(|b| b.channel == format!("event:{event}")).expect("sent to the room");
    assert!(audience.message.contains("Will there be pizza?"), "{}", audience.message);
    assert!(!audience.message.contains("Mark answered"), "no moderation for the audience");
    let host = sent.iter().rev().find(|b| b.channel == format!("host:{event}")).expect("sent to the host");
    assert!(host.message.contains("Mark answered"), "{}", host.message);
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn a_question_has_3_to_280_characters() {
    let (_, event) = host_with_event();
    let mut visitor = Client::new();
    visitor
        .post(&format!("/events/{event}/questions"), &[("body", "hi")])
        .assert_status(422)
        .assert_contains("Body is too short (minimum is 3 characters)");
    let long = "a".repeat(281);
    visitor
        .post(&format!("/events/{event}/questions"), &[("body", long.as_str())])
        .assert_status(422)
        .assert_contains("Body is too long (maximum is 280 characters)");
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn each_browser_votes_once_per_question() {
    let (_, event) = host_with_event();
    let mut first = Client::new().htmx();
    let id = ask(&mut first, &event, "Can we get the slides?");
    first.post(&format!("/questions/{id}/vote"), &()).assert_status(204);
    first.post(&format!("/questions/{id}/vote"), &()).assert_status(204);
    assert_eq!(votes(id), 1);
    Client::new().htmx().post(&format!("/questions/{id}/vote"), &()).assert_status(204);
    assert_eq!(votes(id), 2);
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn the_most_voted_question_comes_first() {
    let (mut host, event) = host_with_event();
    let mut visitor = Client::new().htmx();
    ask(&mut visitor, &event, "First asked");
    let second = ask(&mut visitor, &event, "Second asked");
    visitor.post(&format!("/questions/{second}/vote"), &()).assert_status(204);
    let room = host.get(&format!("/events/{event}"));
    let text = &room.body;
    assert!(text.find("Second asked") < text.find("First asked"), "most voted first");
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn only_the_host_moderates() {
    let (mut host, event) = host_with_event();
    let id = ask(&mut Client::new(), &event, "Off topic?");
    Client::new().post(&format!("/questions/{id}/answer"), &()).assert_redirect_to("/login");
    let (mut other, _) = host_with_event();
    other.post(&format!("/questions/{id}/answer"), &()).assert_status(403);
    other.post(&format!("/questions/{id}/delete"), &()).assert_status(403);

    host.post(&format!("/questions/{id}/answer"), &()).assert_redirect_to(&format!("/events/{event}"));
    assert_eq!(sql(&format!("SELECT answered FROM questions WHERE id = {id}"))[0]["answered"], 1);
    host.post(&format!("/questions/{id}/delete"), &()).assert_redirect_to(&format!("/events/{event}"));
    assert_eq!(sql(&format!("SELECT COUNT(*) AS n FROM questions WHERE id = {id}"))[0]["n"], 0);
}
