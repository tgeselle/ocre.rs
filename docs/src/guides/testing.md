# Testing an Ocre app

An Ocre app is tested today in three layers: native `cargo test` for code that does not touch Cloudflare bindings, `cargo check --target wasm32-unknown-unknown` as the type check of the real build, and end-to-end requests against `ocre dev`. `ocre test` runs all three in one command. This page shows what works, with the exact commands and their output, and lists what Ocre does not provide yet.

## Before you start

- An Ocre app created with `ocre new` (the examples use `ocre new blog --starter blog`, whose `Post` model has `title:string`, `body:text` and `published:boolean`).
- Rust installed with rustup; the app's `rust-toolchain.toml` adds the `wasm32-unknown-unknown` target.
- Node.js 20 or newer, for `ocre dev` (it runs `npx wrangler@4`).
- No generator is needed: the tests below are added by hand.

## What you can test, and where

| Code | Native `cargo test` | `ocre dev` + HTTP requests |
|---|---|---|
| Pure functions (formatting, parsing, slugs, prices) | yes | yes |
| A model's `validate()` on `New<Model>` / `<Model>Changes` | yes | yes |
| Handlers without `State(ctx)` (`home`, `up`), askama templates | yes | yes |
| `ocre::password`, `ocre::token`, `ocre::jwt::{encode_with, decode_with}` | yes | yes |
| Model queries (`create`, `find`, `update`...), uniqueness and foreign-key checks | no | yes |
| Handlers taking `State(ctx)`, `Session`, `Flash`, `CurrentUser`, `BearerUser` | no | yes |
| CSRF, CORS and security headers (`ocre::serve`) | no | yes |
| Mailer functions (the `Email` they build), job structs, `ocre::mail::address_with_name` | yes | yes |
| Sending mail, running jobs and crons, R2 files, KV cache, realtime | no | yes |

Everything that reaches D1, KV, R2, Queues, Durable Objects or email goes through `ocre::Ctx`, and only `ocre::serve` (and the `queue`, `scheduled` and `email` entry points) can build one: a `Ctx` wraps the Worker's JavaScript environment, which exists only inside workerd. So that code runs in `ocre dev`, not in `cargo test`.

## What Ocre does not provide yet

- `ocre new` and the generators write no tests and no `tests/` directory.
- There is no test client for handlers, no in-memory or test D1 database, and no fixtures or factories.
- Integration tests in `tests/*.rs` cannot see the app. A generated app is a `cdylib` (the WebAssembly module), so Cargo has no library to link them to; a file `tests/external.rs` that names the crate fails with:

```text
error[E0433]: cannot find module or crate `blog` in this scope
 --> tests/external.rs:3:13
  |
3 |     let _ = blog::models::post::NewPost { title: "a".into(), body: "b".into(), published: false };
  |             ^^^^ use of unresolved module or unlinked crate `blog`
```

Put unit tests in a `#[cfg(test)] mod tests` inside the file they test, as below.

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

Handlers and Ocre's password functions are `async`. The app has no async runtime outside workerd, so add a small executor as a development dependency (dev-dependencies are not part of the WebAssembly build):

```sh
cargo add --dev pollster@1
```

Then block on the future in the test. This module, at the end of `src/lib.rs`, tests the starter's `home` and `up` handlers and the functions `ocre g auth` builds on:

```rust
// at the end of src/lib.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_page_renders() {
        let Html(page) = pollster::block_on(home()).unwrap();
        assert!(page.contains("<h1>blog</h1>"), "{page}");
    }

    #[test]
    fn up_answers_ok() {
        assert_eq!(pollster::block_on(up()), "OK");
    }

    #[test]
    fn passwords_hash_natively() {
        let digest = pollster::block_on(ocre::password::hash("correct horse")).unwrap();
        assert!(pollster::block_on(ocre::password::verify("correct horse", &digest)).unwrap());
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

## All checks in one command: ocre test

`ocre test` runs `cargo test`, then `cargo check --target wasm32-unknown-unknown`, and stops at the first failure. Arguments after `--` go to `cargo test`:

```sh
ocre test                  # both steps
ocre test -- post::tests   # only the tests whose name contains post::tests
```

```text
...
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.52s
  cargo test: ok
  cargo check --target wasm32-unknown-unknown: ok
```

It exits with status 1 when a step fails, so it can run in CI; `ocre test --json` prints `{"command":"test","ok":true,"ran":["cargo test: ok","cargo check --target wasm32-unknown-unknown: ok"]}` on stdout and cargo's output on stderr. With `--e2e` it also runs your end-to-end script against a server it starts (see [below](#run-the-script-with-ocre-test-e2e)).

## End-to-end tests against ocre dev

`ocre dev` applies local migrations and runs the app in workerd, the runtime Cloudflare uses in production, with a local D1 database, queues, R2 and KV under `.wrangler/state`. Everything the unit tests cannot reach is tested here, with HTTP requests.

### Start the app

```sh
ocre dev                # http://localhost:8787
ocre dev --port 8805    # another port, e.g. for a second app
```

Wait for wrangler's `Ready on http://localhost:8787` line (the first build compiles every dependency to WebAssembly: a minute or two).

### Send requests with curl

```sh
curl -s http://localhost:8787/up
```

```text
OK
```

Create a post and keep only the headers that matter:

```sh
curl -si -X POST http://localhost:8787/posts -d 'title=Hello&body=First+post&published=true' \
  | grep -iE '^(HTTP|location|set-cookie)'
```

```text
HTTP/1.1 303 See Other
Location: /posts/1
Set-Cookie: _ocre_session=UXRzDZDuYNFoQp1fytfMPOeFjEXsJ4lhsLxC3bcIDa0Hfe9ziNyrcfnA8s0dWm80OUDst+yDXXHOjD9xt7QsKGrxxsjg5bdbmPHu3vBQ9nqNtQ%3D%3D; HttpOnly; SameSite=Lax; Path=/
```

The session cookie carries the flash message "Post was successfully created." to the next page: send it back with `-H 'Cookie: _ocre_session=...'` to see it.

A failed validation is a 422 with the messages in the page:

```sh
curl -s -X POST http://localhost:8787/posts -d 'title=&body=' | grep '<li>'
```

```text
    <li>Title can&#39;t be blank</li><li>Body can&#39;t be blank</li>
```

curl sends neither `Sec-Fetch-Site` nor `Origin`, so Ocre's CSRF check lets it through, as it does any non-browser client. To test the check, send the header a browser sends for a form posted from another site:

```sh
curl -si -X POST http://localhost:8787/posts -H 'Sec-Fetch-Site: cross-site' -d 'title=x&body=y'
```

```text
HTTP/1.1 403 Forbidden
Transfer-Encoding: chunked
Content-Type: text/plain; charset=utf-8
referrer-policy: strict-origin-when-cross-origin
x-content-type-options: nosniff
x-frame-options: SAMEORIGIN
x-permitted-cross-domain-policies: none
x-xss-protection: 0

Forbidden: cross-site request. Add the origin to ALLOWED_ORIGINS to allow it.
```

### Inspect the database with ocre sql

`ocre sql` runs SQL on the same local database, also while `ocre dev` runs:

```sh
ocre sql "SELECT id, title, published FROM posts"
ocre sql "SELECT id, title FROM posts" --json
```

```text
id | title | published
---+-------+----------
1  | Hello | 1
(1 row)
{"command":"sql","ok":true,"rows":[{"meta":{"duration":0},"results":[{"id":1,"title":"Hello"}],"success":true}]}
```

With `--json`, the rows are in `rows[0].results` (one entry per statement), which scripts can compare with `jq`.

### Start from a known state with ocre db reset

`ocre db reset` deletes the local database (`.wrangler/state/v3/d1`), applies every migration, then runs `db/seeds.sql` when the app has one. Put the data your tests expect in the seeds:

```sql
-- db/seeds.sql: known data for local tests
INSERT INTO posts (title, body, published) VALUES ('Seeded post', 'From db/seeds.sql', 1);
```

Stop `ocre dev` first. A running `ocre dev` keeps the deleted database open, and every query then fails until it restarts: requests answer 500 and the log shows `✘ [ERROR] [ocre] D1 query failed: Error: internal error; ...`. The order is:

```sh
# 1. stop `ocre dev` (Ctrl-C)
ocre db reset
# 2. start `ocre dev` again
```

```text
 ⛅️ wrangler 4.143.0
...
┌───────────────────────┬────────┐
│ name                  │ status │
├───────────────────────┼────────┤
│ 0001_create_posts.sql │ ✅     │
└───────────────────────┴────────┘
...
🚣 1 command executed successfully.
...
  deleted .wrangler/state/v3/d1
  applied migrations (--local)
  loaded db/seeds.sql (--local)
```

With `--json`, wrangler's output goes to stderr and stdout holds one object listing the same steps, for example `{"command":"db reset","ok":true,"ran":["deleted .wrangler/state/v3/d1","applied migrations (--local)"]}` for an app without seeds. `ocre db reset` only touches the local database; it has no `--remote` flag.

### A smoke-test script

A shell script with curl is enough to check the important paths after each change. Status codes are the most stable thing to assert:

```sh
#!/bin/sh
# script/smoke.sh: end-to-end checks against a running `ocre dev`.
# Usage: BASE=http://localhost:8787 sh script/smoke.sh
set -eu
BASE=${BASE:-http://localhost:8787}
failures=0

# expect <description> <expected> <actual>
expect() {
  if [ "$2" = "$3" ]; then
    echo "ok   $1"
  else
    echo "FAIL $1: expected '$2', got '$3'"
    failures=$((failures + 1))
  fi
}

status() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

expect "health check" 200 "$(status "$BASE/up")"
expect "seeded post is listed" 1 "$(curl -s "$BASE/posts" | grep -c 'Seeded post')"
expect "create redirects" 303 "$(status -X POST "$BASE/posts" -d 'title=Smoke&body=test')"
expect "blank title is refused" 422 "$(status -X POST "$BASE/posts" -d 'title=&body=x')"
expect "cross-site form is refused" 403 \
  "$(status -X POST "$BASE/posts" -H 'Sec-Fetch-Site: cross-site' -d 'title=x&body=y')"
expect "missing post is 404" 404 "$(status "$BASE/posts/999")"

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
```

After `ocre db reset` and a fresh `ocre dev`:

```sh
sh script/smoke.sh
```

```text
ok   health check
ok   seeded post is listed
ok   create redirects
ok   blank title is refused
ok   cross-site form is refused
ok   missing post is 404
all checks passed
```

The script exits with status 1 when a check fails, so an agent or a CI job can run it.

### Run the script with ocre test --e2e

`ocre test --e2e` runs the whole chain unattended: `cargo test`, the wasm32 check, then local migrations, one `wrangler dev` on port 8788 (`--port` to change it) started for the run, and `sh tests/e2e.sh` with `BASE_URL=http://localhost:8788` once the server is ready. The server stops when the script ends, and the command fails when the script exits non-zero. Save the script above as `tests/e2e.sh`, reading `BASE_URL` instead of `BASE`:

```sh
BASE=${BASE_URL:-http://localhost:8787}
```

The server uses the same local database as `ocre dev`: run `ocre db reset` first when the checks expect the seeds. Without `tests/e2e.sh`, `ocre test --e2e` stops before running anything with ``error: tests/e2e.sh not found``. See [ocre test](../reference/cli.md#ocre-test).

### Emails, jobs and scheduled tasks

These leave traces in the `ocre dev` output rather than in responses:

- Emails: with `MAIL_ADAPTER=log` (written to `.dev.vars` by `ocre new`), each email is printed between `[ocre mail]` lines, links included, and the last 20 are listed as JSON at `http://localhost:8787/ocre/dev/mailers/sent.json`: `curl -s http://localhost:8787/ocre/dev/mailers/sent.json | jq -r '.[-1].email.text'` reads the magic-link or reset token of the last one. Mailer previews are at `/ocre/dev/mailers` (see [Email](email.md#preview-and-inspect-emails-in-development)).
- Jobs: they run within about 5 seconds of being enqueued; look for `[ocre jobs] <job> done` (see [Background jobs and schedules](jobs.md)).
- Scheduled tasks: fire one with `ocre schedules run <task>` (or `curl 'http://localhost:8787/cdn-cgi/local/scheduled?cron=0+3+*+*+*'`, the cron expression URL-encoded) and read the `[ocre cron]` lines.
- Incoming email: the form at `http://localhost:8787/ocre/dev/mailbox`, or POST a raw message to `http://localhost:8787/cdn-cgi/local/email?from=...&to=...` (see [Email](email.md#test-it-locally)).

A test script can redirect `ocre dev`'s output to a file and wait for the line it expects.

## How the Ocre repository tests generated apps

Ocre's own end-to-end suite, `crates/ocre-cli/tests/system/e2e.rs`, is a working example of the approach above written in Rust. Each test:

1. creates an app with the real CLI (`ocre new e2e --starter blog`, then generators such as `ocre g scaffold Book ...`);
2. runs `ocre dev --port <free port>` in its own process group, with stdout and stderr in a log file, and polls the base URL until it answers (up to 240 seconds for the first build);
3. sends requests with the `ureq` HTTP client, redirects turned off, and asserts on status codes, `Location`, headers and HTML (for example `<li>Title can&#39;t be blank</li>` on a 422);
4. carries the `_ocre_session` cookie from one response to the next like a browser, to test flash messages, sign-in and CSRF (`sec-fetch-site: cross-site` must get 403);
5. reads the log file for what has no HTTP answer: emailed tokens printed by the `log` mail adapter, `[ocre jobs]` and `[ocre cron]` lines;
6. kills the process group when done.

The start-up part, from the file:

```rust
fn start(sandbox: &Sandbox, root: &Path) -> Server {
    let port = free_port().to_string();
    let mut command = sandbox.command(&["dev", "--port", &port], root);
    // ...
    // One target dir for every run, so the wasm dependencies compile once.
    command.env("CARGO_TARGET_DIR", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/e2e-app"));
    let log = sandbox.work.join("dev.log");
    let output = std::fs::File::create(&log).unwrap();
    let child = command.process_group(0).stdout(output.try_clone().unwrap()).stderr(output).spawn().unwrap();
    // wrangler dev listens on `localhost`, which is IPv6-only on some Linux hosts.
    let mut server = Server { child, base: format!("http://localhost:{port}") };
    let deadline = Instant::now() + Duration::from_secs(240);
    while agent().get(&server.base).call().is_err() {
        // ...
        std::thread::sleep(Duration::from_secs(1));
    }
    server
}
```

In the Ocre repository, it runs with:

```sh
cargo test -p ocre-cli --test e2e -- --ignored
```

To do the same for your app, put such a harness in a separate Cargo project next to it (the app itself is a `cdylib`), pointed at an already created app instead of generating one. Sharing one `CARGO_TARGET_DIR` between runs, as above, keeps the WebAssembly build incremental.

## Reference

- [CLI commands](../reference/cli.md): [`ocre test`](../reference/cli.md#ocre-test), [`ocre dev`](../reference/cli.md#ocre-dev), [`ocre db reset`](../reference/cli.md#ocre-db-reset), [`ocre db seed`](../reference/cli.md#ocre-db-seed), [`ocre sql`](../reference/cli.md#ocre-sql)
- [Validations](validations.md): what `validate()`, `create` and `update` check
- [Sessions, flash and security](security.md): the CSRF check the smoke test exercises
- [Email](email.md), [Background jobs and schedules](jobs.md): what to read in the `ocre dev` output
- [Architecture](../explanations/architecture.md): why bindings only exist inside workerd
- The `ocre` crate's [rustdoc](/api/ocre/index.html): `ocre::password`, `ocre::jwt::{encode_with, decode_with}`, `ocre::Validator`
