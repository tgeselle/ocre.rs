# Testing an Ocre app

An Ocre app is tested in four layers: native `cargo test` for code that touches no Cloudflare binding, `cargo check --target wasm32-unknown-unknown` for the real build, request tests that talk HTTP to the app running in workerd, and browser tests (Playwright) for pages with JavaScript. Generators write the tests for what they generate; `ocre test` runs the first two layers, `ocre test --e2e` all four, against a fresh test database.

## Before you start

- An Ocre app created with `ocre new` (the examples use `ocre new blog --starter blog`, whose `Post` model has `title:string`, `body:text` and `published:boolean`).
- Rust installed with rustup; the app's `rust-toolchain.toml` adds the `wasm32-unknown-unknown` target.
- Node.js 22 or newer and the app's npm packages (`npm install`, run by `ocre new`).

## What you can test, and where

| Code | Native `cargo test` | Request tests (`ocre test --e2e`) |
|---|---|---|
| Pure functions, a model's `validate()`, mailer functions (the `Email` they build), askama templates | yes | yes |
| `ocre::password`, `ocre::token`, `ocre::jwt::{encode_with, decode_with}` | yes | yes |
| Model queries, uniqueness and reference checks, callbacks | no | yes |
| Handlers with `State(ctx)`, `Session`, `Flash`, `Cookies`, `CurrentUser`; CSRF and security headers | no | yes |
| Sending mail, jobs, crons, R2 files, KV cache, realtime broadcasts, mailboxes | no | yes |
| Pages whose behavior needs JavaScript (htmx, Trix, uploads) | no | browser tests |

Everything that reaches D1, KV, R2, Queues, Durable Objects or email goes through `ocre::Ctx`, which only exists inside workerd, so request tests reach it over HTTP.

## The tests generators write

| File | Written by | What it holds |
|---|---|---|
| `tests/app.rs` | `ocre new` | `/up` and the home page answer |
| `tests/<plural>.rs` | `ocre g scaffold` | list, show, create, update, delete and a rejected invalid record |
| `tests/api_<plural>.rs` | `ocre g api` | the same through the JSON API |
| `tests/factories/<model>.rs` | `ocre g model` (and scaffold, api) | valid, unique attributes: `post()`, `.insert()`, `.form()`, `.json()` |
| `tests/fixtures/<table>.yml` | you, or `ocre db dump --dir tests/fixtures` | named rows loaded into the test database before each run |
| `tests/system/<name>.spec.ts` | `ocre g system_test <name>` | a browser test |

A generated request test:

```rust,ignore
// tests/posts.rs
mod factories;

use factories::post::post;
use ocre::testing::Client;

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn creates_a_post() {
    let mut client = Client::new();
    let created = client.post("/posts", &post().form());
    let location = created.assert_status(303).location().unwrap_or_default().to_owned();
    assert!(location.starts_with("/posts/"), "redirects to the new post: {location}");
    assert_eq!(client.flash("notice").as_deref(), Some("Post was successfully created."));
    client.follow_redirect(&created).assert_status(200).assert_contains("Post was successfully created.");
}
```

`#[ignore]` marks the tests that need the running app: plain `cargo test` (and `ocre test`) skips them, `ocre test --e2e` runs them.

## Unit tests with cargo test

`cargo test` builds the app for your machine (not WebAssembly) and runs the `#[test]` functions. The `worker` crate and Ocre compile natively, so the whole app builds; only code that calls the Workers runtime cannot run.

### Test a pure function

Keep logic that needs no binding in plain functions: they are the cheapest code to test.

```rust,check
// src/slug.rs (declare it with `mod slug;` under `// ocre:modules` in src/lib.rs)

/// `"Hello, Edge World!"` -> `"hello-edge-world"`: lowercase ASCII letters and
/// digits, words joined by `-`.
pub fn slugify(title: &str) -> String {
    title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn joins_lowercase_words_with_dashes() {
        assert_eq!(slugify("Hello, Edge World!"), "hello-edge-world");
    }

    #[test]
    fn drops_punctuation_and_non_ascii_letters() {
        assert_eq!(slugify("  --  "), "");
        assert_eq!(slugify("Crème brûlée"), "cr-me-br-l-e");
    }
}
```

### Test a model's validations

A generated model's `validate()` needs no database: it returns an `ocre::Validator`, and `finish()` gives `Error::Invalid` with every message. Add a test module at the end of `src/models/post.rs`:

```rust
// at the end of src/models/post.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_post_needs_a_title_and_a_body() {
        let new = NewPost { title: " ".into(), body: String::new(), published: false };
        let err = new.validate().finish().unwrap_err();
        assert_eq!(err.to_string(), "invalid: Title can't be blank, Body can't be blank");
    }

    #[test]
    fn changes_only_check_the_fields_they_set() {
        assert!(PostChanges::default().validate().finish().is_ok());
    }
}
```

The uniqueness (`^`) and foreign-key (`references`) checks run in `create` and `update`, against D1: test them end to end.

```sh
cargo test
```

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.16s
     Running unittests src/lib.rs (target/debug/deps/blog-e620362bcd833d70)

running 4 tests
test models::post::tests::changes_only_check_the_fields_they_set ... ok
test slug::tests::drops_punctuation_and_non_ascii_letters ... ok
test models::post::tests::a_post_needs_a_title_and_a_body ... ok
test slug::tests::joins_lowercase_words_with_dashes ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The first `cargo test` compiles every dependency natively (19 seconds on the machine these docs were written on); later runs take a second or two.

### Test async code

Handlers and Ocre's password functions are `async`. The app has no async runtime outside workerd; `ocre::testing::block_on` (the `testing` feature a generated app enables in `[dev-dependencies]`) runs a future on the test thread. This module, at the end of `src/lib.rs`, tests the starter's `home` and `up` handlers and the functions `ocre g auth` builds on:

```rust
// at the end of src/lib.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_page_renders() {
        let Html(page) = ocre::testing::block_on(home()).unwrap();
        assert!(page.contains("<h1>blog</h1>"), "{page}");
    }

    #[test]
    fn up_answers_ok() {
        assert_eq!(ocre::testing::block_on(up()), "OK");
    }

    #[test]
    fn passwords_hash_natively() {
        let digest = ocre::testing::block_on(ocre::password::hash("correct horse")).unwrap();
        assert!(ocre::testing::block_on(ocre::password::verify("correct horse", &digest)).unwrap());
    }

    #[test]
    fn jwt_round_trip() {
        use ocre::jwt::{Claims, Key, decode_with, encode_with};
        let key = Key::from_secret_key_base(&"x".repeat(64));
        let token = encode_with(&key, &Claims::new("42", 60));
        assert_eq!(decode_with(&key, &token, ocre::now()).unwrap().sub, "42");
    }
}
```

```sh
cargo test
```

```text
running 8 tests
test tests::up_answers_ok ... ok
test models::post::tests::changes_only_check_the_fields_they_set ... ok
test slug::tests::drops_punctuation_and_non_ascii_letters ... ok
test slug::tests::joins_lowercase_words_with_dashes ... ok
test tests::home_page_renders ... ok
test models::post::tests::a_post_needs_a_title_and_a_body ... ok
test tests::jwt_round_trip ... ok
test tests::passwords_hash_natively ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.99s
```

Natively, `ocre::password` computes the same PBKDF2-HMAC-SHA256 digest in pure Rust instead of WebCrypto, with 100,000 iterations. In an unoptimized test build, `passwords_hash_natively` (one hash, one verify) alone took 5.39 seconds; with `cargo test --release passwords` it took 0.11 seconds. Keep such tests few, or run them in release mode. `ocre::jwt::encode` and `decode` read `SECRET_KEY_BASE` from a `Ctx`; tests use `encode_with` and `decode_with` with a `Key` instead.

## Type-check the WebAssembly build

`cargo test` compiles for your machine; the Worker is built for `wasm32-unknown-unknown`, where some code differs (`#[cfg(target_arch = "wasm32")]` in Ocre and the `worker` crate). Check the real target after every change:

```sh
cargo check --target wasm32-unknown-unknown
```

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 22.52s
```

This check does not compile `#[cfg(test)]` code, and `cargo test` does not compile for WebAssembly: run both. A function used only by tests shows up here as a `dead_code` warning ("function `slugify` is never used") until app code calls it.

## Request tests: ocre test --e2e

`ocre test --e2e` runs, in order and stopping at the first failure:

1. `cargo test` and the wasm32 check;
2. a fresh test database in `.wrangler/test-state` (the development data in `.wrangler/state` is untouched): migrations, then `tests/fixtures/*.yml`;
3. one server for the whole run (the app's wrangler, as `cf dev` runs it) on port 8788 (`--port` to change it), logging to `.wrangler/test-state/dev.log`;
4. `cargo test -- --ignored --test-threads=1`, with `OCRE_TEST_URL`, `OCRE_TEST_STATE` and `OCRE_TEST_LOG` set for `ocre::testing`;
5. `tests/e2e.sh` with `BASE_URL`, when the app has one;
6. the browser tests of `tests/system/`, when there are some.

```sh
ocre test --e2e
ocre test --e2e -- posts             # only request tests whose name contains "posts"
ocre test --e2e --json               # one JSON report on stdout, tool output on stderr
```

```text
{"command":"test","ok":true,"ran":["cargo test: ok","cargo check --target wasm32-unknown-unknown: ok","test database .wrangler/test-state: migrated, tests/fixtures loaded","cargo test -- --ignored against the test server on port 9555: ok","playwright test (tests/system) against the test server on port 9555: ok"]}
```

Tests share one database and run one at a time: create your own records (factories give unique values) and assert on them, not on global counts. There are no per-test transactions: the database belongs to the server process.

### The client

`ocre::testing::Client` sends requests to the test server and keeps cookies like a browser (the session, its flash messages, your own cookies):

| Method | Does |
|---|---|
| `get`, `post(path, &form)`, `post_json`, `patch_json`, `put_json`, `delete`, `request` | send a request; forms are URL-encoded |
| `follow_redirect(&response)` | GET the `Location` |
| `header(name, value)`, `cross_site()`, `htmx()` | extra headers (a cross-site form, an htmx request) |
| `session()`, `flash(kind)`, `cookie(name)`, `set_cookie(name, value)` | read the decrypted session, the pending flash, a cookie |
| `deliveries()` | emails the app sent (`MAIL_ADAPTER=log`), from `/ocre/dev/mailers/sent.json` |
| `broadcasts()` | realtime messages sent (`/ocre/dev/realtime/sent.json`) |
| `jobs()` | jobs enqueued and those the local queue ran, with `done`, `discarded` or `retried` (`/ocre/dev/jobs.json`, merged by the first `ocre g job`) |
| `receive_email(from, to, subject, body)` | delivers a message to the app's mailbox (`ocre g mailbox`) through the local Email Routing endpoint |

A `Response` has `status`, `headers`, `body` and the assertions `assert_status`, `assert_success`, `assert_redirect_to`, `assert_contains`, `assert_not_contains` and `assert_header`, each returning the response for chaining.

```rust,ignore
use ocre::testing::{Client, eventually};

#[test]
#[ignore = "request test: run with `ocre test --e2e`"]
fn signing_up_sends_a_welcome_email_from_a_job() {
    let mut client = Client::new();
    client.post("/signups", &[("email", "ada@example.com")]).assert_status(303);
    assert_eq!(client.jobs().enqueued.last().unwrap().name(), Some("send_welcome"));
    // Local queues deliver within a few seconds.
    eventually(|| client.jobs().performed.iter().any(|run| run.job == "send_welcome" && run.outcome == "done").then_some(()));
    assert_eq!(client.deliveries().last().unwrap().to, ["ada@example.com"]);
}
```

### Test data: factories and fixtures

A factory builds valid attributes, unique per call, and writes them with one call to the test database:

```rust,ignore
let id = post().insert();                                             // a row
let draft = factories::post::Post { published: false, ..post() }.insert(); // with changes
client.post("/posts", &post().form());                                // as a form
```

Factories of models with `references` create the parent first (`with_parents`). Fixtures are named rows for data every test can rely on, in Rails' format:

```yaml
# tests/fixtures/posts.yml
DEFAULTS: &defaults
  published: true
hello:
  <<: *defaults
  title: Hello $LABEL      # $LABEL is the row's label
  author: ada              # author_id = id of the fixture labelled `ada` in users.yml
```

Read them with `ocre::testing::fixture_id("hello")` and `fixture("posts", "hello")`. `ocre::testing::sql(query)`, `count(table)` and `insert(table, values)` reach the test database directly.

### Assertions, the server log and time

- `assert_difference(|| count("posts"), 1, || { ... })`, `assert_no_difference`, `assert_changes`, `assert_no_changes`: Rails' names.
- `Log::mark()` then `log.wait_for("[ocre jobs] send_welcome done")`: lines the server printed since the mark.
- `eventually(|| ...)`: retries a check for a few seconds (queues, broadcasts).
- `travel_to(unix)`, `travel(seconds)`, `freeze_time()`, `travel_back()`: `ocre::now()` in the test process (native code under test, such as token expiry).
- `redact(text)`: replaces ids, dates and tokens, for stable snapshots.

### A shell script: tests/e2e.sh

Checks that are easier with curl go in `tests/e2e.sh`, run with `BASE_URL` set:

```sh
#!/bin/sh
set -eu
status() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
[ "$(status "$BASE_URL/up")" = 200 ]
[ "$(status -X POST "$BASE_URL/posts" -H 'Sec-Fetch-Site: cross-site' -d 'title=x&body=y')" = 403 ]
```

## Browser tests: ocre g system_test

Pages that need JavaScript (htmx swaps, the rich text editor, direct uploads) are tested in a real browser with [Playwright](https://playwright.dev), Rails' system tests:

```sh
ocre g system_test creating_a_post
npm install                            # @playwright/test, added to package.json
npx playwright install chromium        # once per machine
```

The first run writes `playwright.config.ts`: tests in `tests/system/`, the base URL from `BASE_URL`, one worker (the database is shared), each test on a desktop and a phone screen, and a screenshot and trace of each failure in `test-results/` (git-ignored). Edit the generated test:

```ts
// tests/system/creating_a_post.spec.ts
import { expect, test } from "@playwright/test";

test("Creating a post", async ({ page }) => {
  await page.goto("/posts/new");
  await page.getByLabel("Title").fill("Hello from Playwright");
  await page.getByRole("button", { name: /create|save/i }).click();
  await expect(page.getByText("Post was successfully created.")).toBeVisible();
});
```

`ocre test --e2e` runs them after the request tests, with `node_modules/.bin/playwright test`. Assertions wait for the page, so no sleeps are needed. Other browsers are more `projects` in the config (`devices["Desktop Firefox"]`, then `npx playwright install firefox`). Against `ocre dev` instead: `BASE_URL=http://localhost:8787 npx playwright test --ui`.

Errors: `tests/system has tests but Playwright is not installed` (run `npm install`), and `playwright test failed (exit status: 1)` with a hint pointing to `test-results/`.

## Manual checks against ocre dev

`ocre dev` runs the app with the development data. `ocre sql "SELECT ..."` reads the same database while it runs, `ocre db reset` (with `ocre dev` stopped) recreates it from the migrations and `db/seeds.sql`, and the development pages show what has no HTTP answer: `/ocre/dev/mailers` (emails), `/ocre/dev/mailbox` (deliver an email), `/ocre/dev/jobs.json` (jobs), `/ocre/dev/realtime/sent.json` (broadcasts).

## CI

`ocre ci` runs `cargo fmt --check`, clippy with `-D warnings`, `cargo test`, the wasm32 check and `ocre i18n missing`; `ocre g ci` writes the same steps as a GitHub Actions workflow. Generators format the Rust they write with the app's `rustfmt.toml`, so a freshly generated app passes. Add `ocre test --e2e` to the workflow to run the request and browser tests too (the runner needs Node.js and, for browser tests, `npx playwright install --with-deps chromium`).

## Reference

- [CLI commands](../reference/cli.md): [`ocre test`](../reference/cli.md#ocre-test), [`ocre ci`](../reference/cli.md#ocre-ci), [`ocre db dump`](../reference/cli.md#ocre-db-dump), [`ocre sql`](../reference/cli.md#ocre-sql)
- [Generators](../reference/generators.md): factories, `ocre g system_test`
- [Email](email.md), [Background jobs and schedules](jobs.md), [Realtime](realtime.md): what the captures list
- The `ocre::testing` [rustdoc](/api/ocre/testing/index.html)
