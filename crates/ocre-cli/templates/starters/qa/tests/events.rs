//! Request tests for events: hosts create them, their page is the room.
//! They run against the app in workerd with `ocre test --e2e`; plain
//! `cargo test` skips them (`#[ignore]`).

use ocre::testing::{Client, sequence};

/// A signed-up host.
fn host() -> Client {
    let mut client = Client::new();
    // Unique across test files, which share the test database.
    let email = format!("host{}-{}@example.com", std::process::id(), sequence());
    client.post("/signup", &[("email", email.as_str()), ("password", "correct horse")]).assert_status(303);
    client
}

/// `client` creates an event; returns its public id.
fn create_event(client: &mut Client, name: &str) -> String {
    let created = client.post("/events", &[("name", name)]);
    let location = created.assert_status(303).location().unwrap_or_default().to_owned();
    location.strip_prefix("/events/").expect("redirects to the room").to_owned()
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn a_host_creates_an_event_and_gets_its_link() {
    let mut client = host();
    let id = create_event(&mut client, "Friday all-hands");
    client
        .get(&format!("/events/{id}"))
        .assert_status(200)
        .assert_contains("Friday all-hands")
        .assert_contains(&format!("/events/{id}</code>"))
        .assert_contains("You host this event");
    client.get("/events").assert_status(200).assert_contains("Friday all-hands");
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn visitors_join_the_room_without_an_account() {
    let mut client = host();
    let id = create_event(&mut client, "Open room");
    Client::new()
        .get(&format!("/events/{id}"))
        .assert_status(200)
        .assert_contains("Open room")
        .assert_contains(&format!("ws-connect=\"/realtime/event:{id}\""))
        .assert_not_contains("You host this event");
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn creating_an_event_needs_an_account() {
    Client::new().post("/events", &[("name", "Sneaky")]).assert_redirect_to("/login");
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn only_the_host_changes_an_event() {
    let mut owner = host();
    let id = create_event(&mut owner, "Mine");
    let mut other = host();
    other.post(&format!("/events/{id}"), &[("name", "Theirs")]).assert_status(403);
    other.post(&format!("/events/{id}/delete"), &()).assert_status(403);
    owner.post(&format!("/events/{id}"), &[("name", "Still mine")]).assert_status(303);
    owner.get(&format!("/events/{id}")).assert_contains("Still mine");
    owner.post(&format!("/events/{id}/delete"), &()).assert_redirect_to("/events");
    owner.get(&format!("/events/{id}")).assert_status(404);
}
