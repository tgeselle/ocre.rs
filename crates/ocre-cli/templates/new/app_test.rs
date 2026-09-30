//! Request tests: HTTP requests to the app running in workerd, with a fresh
//! local database (migrations + tests/fixtures/*.yml). `ocre test --e2e`
//! starts one server for the whole run and runs them; plain `cargo test`
//! skips them (`#[ignore]`). Helpers: `ocre::testing` (client, assertions,
//! test database, time travel); test data: tests/factories/ (`ocre g model`).

use ocre::testing::Client;

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn up_answers_ok() {
    Client::new().get("/up").assert_status(200).assert_contains("OK");
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn home_answers() {
    Client::new().get("/").assert_status(200).assert_contains("__APP_NAME__");
}

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn unknown_paths_are_not_found() {
    Client::new().get("/no-such-page").assert_status(404);
}
