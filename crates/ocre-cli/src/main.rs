//! `ocre`: command-line tool for Ocre apps.
//!
//! Commands never prompt unless `ocre new` runs in a terminal without
//! `--json`/`--yes`. With `--json`, stdout carries exactly one JSON object
//! (`{"ok": true, ...}` or `{"ok": false, "error", "hint"}`) and all tool
//! output (cf, wrangler, cargo, npm) goes to stderr.

mod about;
mod ci;
mod cloudflare;
mod config;
mod db;
mod db_admin;
mod destroy;
mod doctor;
mod domains;
mod fixtures;
mod generate;
mod i18n;
mod names;
mod new;
mod notes;
mod output;
mod project;
mod routes;
mod schedules;
mod secret;
mod secrets;
mod stats;
mod template;
mod testing;
mod wizard;

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};

use generate::{Existing, GenerateOptions};
use new::{NewArgs, Starter};
use output::{CliError, Report};
use project::Project;

#[derive(Parser)]
#[command(
    name = "ocre",
    version,
    about = "Ocre: Rails-like Rust web framework for Cloudflare Workers, free plan first."
)]
struct Cli {
    /// Print one JSON object on stdout; send tool logs to stderr. Never prompts.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new app in ./<NAME>. In a terminal, asks for anything not given by flags.
    New {
        /// App name: lowercase letters, digits and dashes (e.g. `my-blog`).
        name: Option<String>,
        /// API only: JSON endpoints, no HTML templates (like `rails new --api`).
        #[arg(long, overrides_with = "full_stack")]
        api: bool,
        /// HTML pages with askama and htmx, plus JSON APIs when generated.
        #[arg(long)]
        full_stack: bool,
        /// Starter content.
        #[arg(long, value_enum)]
        starter: Option<Starter>,
        /// Log in to Cloudflare (opens a browser) if not logged in yet.
        #[arg(long, overrides_with = "no_login")]
        login: bool,
        /// Do not offer the Cloudflare login.
        #[arg(long)]
        no_login: bool,
        /// Cloudflare account to deploy to, when the login has several.
        #[arg(long)]
        account_id: Option<String>,
        /// Run `git init` in the new app.
        #[arg(long, overrides_with = "no_git")]
        git: bool,
        #[arg(long)]
        no_git: bool,
        /// Deploy right after creating the app (implies --login).
        #[arg(long, overrides_with = "no_deploy")]
        deploy: bool,
        #[arg(long)]
        no_deploy: bool,
        /// Skip `npm install` (cf, wrangler, typescript) in the new app; run
        /// it before `ocre dev`.
        #[arg(long)]
        no_install: bool,
        /// Never prompt; use defaults for anything not given.
        #[arg(long, short)]
        yes: bool,
        /// Use a local checkout of the `ocre` crate instead of the git dependency.
        #[arg(long)]
        ocre_path: Option<PathBuf>,
        /// Application template to apply after creating the app: a file (or
        /// https:// URL) of ocre commands, one per line (see `ocre template --help`).
        #[arg(long, short = 'm')]
        template: Option<String>,
    },
    /// Log in to Cloudflare (opens a browser) unless already logged in.
    Login,
    /// Generate code (alias: `g`). Every run is recorded in .ocre/generated/ for `ocre destroy`.
    #[command(alias = "g")]
    Generate(GenerateArgs),
    /// Undo a generator run (alias: `d`): delete the files it created, take
    /// out the lines it added (Cargo.toml, cloudflare.config.ts and package.json changes stay).
    ///
    /// Example: `ocre destroy scaffold Post`. Refuses when generated files
    /// changed since, unless --force.
    #[command(alias = "d")]
    Destroy {
        /// Generator, as typed after `ocre g` (e.g. `scaffold`).
        generator: String,
        /// Name given to the generator (e.g. `Post`); default: its latest run.
        name: Option<String>,
        /// Delete generated files even if they changed; leave changed lines in place.
        #[arg(long)]
        force: bool,
        /// Show what would be removed; change nothing.
        #[arg(long)]
        pretend: bool,
    },
    /// Apply D1 migrations (local database unless --remote).
    Migrate {
        /// Apply to the production database on Cloudflare.
        #[arg(long)]
        remote: bool,
        /// List pending migrations instead of applying them.
        #[arg(long)]
        status: bool,
    },
    /// Database tasks: create, drop, prepare, seed, reset, truncate, version, schema.
    #[command(subcommand)]
    Db(DbCommand),
    /// Run SQL on the D1 database (local unless --remote) and print the rows.
    ///
    /// Example: `ocre sql "SELECT * FROM posts LIMIT 5"`.
    Sql {
        /// One or more SQL statements separated by `;`.
        query: String,
        /// Run on the production database on Cloudflare.
        #[arg(long)]
        remote: bool,
    },
    /// Apply local migrations, then run the app with `cf dev`.
    ///
    /// Example: `ocre dev --no-cache` to see every `ocre::cache` value computed (like `bin/rails dev:cache`).
    Dev {
        #[arg(long, default_value_t = 8787)]
        port: u16,
        /// Turn `ocre::cache` back on (removes CACHE_STORE from .dev.vars); stays on for later runs.
        #[arg(long, overrides_with = "no_cache")]
        cache: bool,
        /// Turn `ocre::cache` off (CACHE_STORE=null in .dev.vars); stays off for later runs.
        #[arg(long)]
        no_cache: bool,
    },
    /// Deploy to Cloudflare and apply remote migrations.
    Deploy,
    /// Stream the deployed Worker's live logs (`wrangler tail`): each request,
    /// its console lines (`ctx.log()`, Ocre's `[ocre]` errors) and uncaught
    /// exceptions, as they happen. Ctrl-C stops. Stored logs are in the
    /// dashboard (Workers Logs).
    ///
    /// Example: `ocre logs --status error`, `ocre logs --search checkout --format json`.
    Logs {
        /// Output: pretty (default) or json, one object per event.
        #[arg(long, default_value = "pretty", value_parser = ["pretty", "json"])]
        format: String,
        /// Keep invocations with this outcome: ok, error or canceled (repeatable).
        #[arg(long, value_parser = ["ok", "error", "canceled"])]
        status: Vec<String>,
        /// Keep events whose console lines contain this text.
        #[arg(long)]
        search: Option<String>,
    },
    /// Print a new random secret, like `rails secret`: a value for SECRET_KEY_BASE.
    ///
    /// Example: `ocre secret` for a value to put in a git-ignored env file, then `ocre secrets push`.
    Secret,
    /// Worker secrets (Ocre's credentials): list them, or upload values from a
    /// git-ignored env file. Values are encrypted by Cloudflare and never
    /// read back.
    ///
    /// Example: `ocre secrets push GITHUB_CLIENT_SECRET --file .prod.vars`.
    #[command(subcommand)]
    Secrets(SecretsCommand),
    /// List the app's HTTP routes, read from its source (no build).
    ///
    /// Example: `ocre routes posts` keeps routes whose method, path or handler contains "posts".
    Routes {
        /// Only routes whose method, path or handler contains this text (case-insensitive).
        filter: Option<String>,
    },
    /// List the Cron Triggers of cloudflare.config.ts and the task each runs; `run <task>`
    /// fires one on the running `ocre dev`.
    ///
    /// Example: `ocre schedules`, `ocre schedules run nightly_cleanup`.
    Schedules {
        #[command(subcommand)]
        action: Option<SchedulesCommand>,
    },
    /// Translations: checks of the locale files (see `ocre g locale`).
    #[command(subcommand)]
    I18n(I18nCommand),
    /// Apply an application template to this app: a text file (or https://
    /// URL) with one ocre command per line, `#` for comments, e.g.
    /// `g scaffold Post title:string`, `migrate`, `cargo add slug`. Only local
    /// commands are allowed (generators, destroy, migrate, db, sql, i18n,
    /// routes, cargo add/remove; no --remote).
    ///
    /// Example: `ocre template https://example.com/blog.ocre`.
    Template {
        /// Path or URL of the template.
        source: String,
    },
    /// Print the versions of this CLI and of the app.
    Version,
    /// Versions and the app's configuration: Ocre crate and features, Rust
    /// toolchain, compatibility date, bindings and triggers, variable names.
    About,
    /// Check the tools (Rust wasm target, Node.js, Cloudflare login) and the
    /// app (bindings for what the code uses, pending migrations, secrets,
    /// production settings, and the executables in .ocre/doctor/).
    /// Exits with an error when a check fails.
    Doctor,
    /// Run the app's CI steps locally, in order, stopping at the first
    /// failure: `cargo fmt --check`, `cargo clippy --all-targets -- -D
    /// warnings`, `cargo test`, `cargo check --target wasm32-unknown-unknown`,
    /// and `ocre i18n missing` when the app has locales. The same steps as
    /// the workflow of `ocre g ci`.
    ///
    /// Example: `ocre ci --signoff`.
    Ci {
        /// After a green run, `gh signoff` marks the pushed commit as passing
        /// (needs the GitHub CLI and `gh extension install basecamp/gh-signoff`).
        #[arg(long)]
        signoff: bool,
    },
    /// The Worker's custom domains (`worker.domains` of cloudflare.config.ts);
    /// `ocre deploy` publishes it on them. Without a subcommand, list them.
    ///
    /// Example: `ocre domains add www.example.com`, then `ocre deploy`.
    Domains {
        #[command(subcommand)]
        action: Option<DomainsCommand>,
    },
    /// Lines of code per part of the app (models, controllers, templates, tests...).
    ///
    /// Example: `ocre stats lib` also counts lib/.
    Stats {
        /// More directories to count, relative to the app root.
        dirs: Vec<String>,
    },
    /// List TODO, FIXME and OPTIMIZE comments in src/, templates/, migrations/, tests/, db/ and locales/.
    ///
    /// Example: `ocre notes --annotations TODO,HACK`.
    Notes {
        /// Tags to look for instead of TODO, FIXME and OPTIMIZE.
        #[arg(long, value_delimiter = ',')]
        annotations: Vec<String>,
    },
    /// Run the app's tests: `cargo test`, then `cargo check --target
    /// wasm32-unknown-unknown`; with --e2e, then a fresh test database
    /// (migrations + tests/fixtures), one local server for the run, the
    /// request tests (`cargo test -- --ignored`, with OCRE_TEST_URL) and
    /// tests/e2e.sh when present. Stops at the first failure.
    ///
    /// Example: `ocre test --e2e`, or `ocre test -- models` to filter tests.
    Test {
        /// Also run the request tests and tests/e2e.sh against a local server.
        #[arg(long)]
        e2e: bool,
        /// Port of the server started for --e2e.
        #[arg(long, default_value_t = 8788)]
        port: u16,
        /// Arguments passed to `cargo test` (after `--`).
        #[arg(last = true)]
        cargo_args: Vec<String>,
    },
}

#[derive(clap::Args)]
struct GenerateArgs {
    /// Show the files that would be created or updated; write nothing.
    #[arg(long, global = true)]
    pretend: bool,
    /// Overwrite files that already exist.
    #[arg(long, global = true, conflicts_with = "skip")]
    force: bool,
    /// Keep files that already exist and generate the rest.
    #[arg(long, global = true)]
    skip: bool,
    #[command(subcommand)]
    command: GenerateCommand,
}

impl GenerateArgs {
    /// clap leaves an app generator's arguments unparsed, global flags
    /// included: takes them out.
    fn take_custom_flags(&mut self, json: &mut bool) {
        if let GenerateCommand::Custom(extra) = &mut self.command {
            extra.retain(|arg| {
                let flag = match arg.as_str() {
                    "--pretend" => &mut self.pretend,
                    "--force" => &mut self.force,
                    "--skip" => &mut self.skip,
                    "--json" => &mut *json,
                    _ => return true,
                };
                *flag = true;
                false
            });
        }
    }
}

#[derive(Subcommand)]
enum GenerateCommand {
    /// Table, migration and `src/models/<model>.rs` (queries, validations,
    /// associations). Scaffold and api create the model when it is missing.
    ///
    /// Example: `ocre g model Post title:string^ body:text author:references status:enum:draft,published`.
    ///
    /// Types: string, text, integer (int, small_int, big_int), float (double), decimal, boolean (bool),
    /// date, time, datetime (date_time), uuid, references, attachment, json (jsonb), enum:<value>,<value>...;
    /// suffix `?` for optional, `^` for unique; `author:references:writer_id` names the foreign key.
    Model {
        /// Singular model name, PascalCase or snake_case (e.g. `BlogPost`).
        name: String,
        /// Fields as `name:type`.
        #[arg(required = true)]
        fields: Vec<String>,
    },
    /// Model plus HTML pages for full CRUD.
    /// In an API-only app (`ocre new --api`) this is `ocre g api`.
    ///
    /// Example: `ocre g scaffold Post title:string body:text published:boolean --realtime`.
    Scaffold {
        /// Singular model name, PascalCase or snake_case (e.g. `BlogPost`).
        name: String,
        /// Fields as `name:type` (see `ocre g model --help`).
        #[arg(required = true)]
        fields: Vec<String>,
        /// Live index page: creates, edits and deletes appear in every open
        /// browser over a WebSocket (htmx ws extension, Durable Object channel).
        #[arg(long)]
        realtime: bool,
    },
    /// Model plus a JSON REST resource under /api/<plural>; with --graphql, also GraphQL.
    ///
    /// Example: `ocre g api Post title:string body:text --graphql`.
    Api {
        /// Singular model name, PascalCase or snake_case (e.g. `BlogPost`).
        name: String,
        /// Fields as `name:type` (see `ocre g model --help`).
        #[arg(required = true)]
        fields: Vec<String>,
        /// Also expose the resource on /graphql. Adds ~1.1 MB of WebAssembly and
        /// 20-60 ms of CPU when a Worker instance starts.
        #[arg(long)]
        graphql: bool,
    },
    /// Authentication, generated into the app: users (email + password), and in
    /// full-stack apps sign-up/login/logout pages with "remember me", password
    /// reset, magic-link login and email confirmation by email, account
    /// deletion, `CurrentUser`/`ConfirmedUser`/`OptionalUser` extractors
    /// (src/auth.rs); in every app a JSON API with JWTs and API keys and the
    /// `BearerUser` extractor (src/auth_api.rs), and a rate limit on every route
    /// that checks a password or sends an email (`AUTH_RATE_LIMITER` in
    /// cloudflare.config.ts). Runs once per app.
    ///
    /// Example: `ocre g auth --db-sessions --oauth github`, then `ocre migrate`.
    Auth {
        /// Track sessions in D1 (`user_sessions`): /account/sessions lists the
        /// signed-in devices and signs them out. Costs one D1 read per request.
        #[arg(long)]
        db_sessions: bool,
        /// "Continue with ..." sign-in through OAuth providers (github, google),
        /// comma-separated.
        #[arg(long, value_delimiter = ',')]
        oauth: Vec<String>,
    },
    /// Numbered SQL migration. `create_<table>`, `add_<columns>_to_<table>` and
    /// `remove_<columns>_from_<table>` names get their SQL from the fields.
    ///
    /// Example: `ocre g migration add_slug_to_posts slug:string^`.
    Migration {
        /// Migration name in snake_case (e.g. `add_slug_to_posts`).
        name: String,
        /// Columns, as `name:type` (see `ocre g model --help`).
        fields: Vec<String>,
    },
    /// `src/mailers/<name>.rs`: one function per action building an
    /// `ocre::mail::Email`, from askama templates in
    /// `templates/mailers/<name>/<action>.{txt,html}` (text only in API-only apps).
    ///
    /// Example: `ocre g mailer User welcome password_reset`.
    Mailer {
        /// Mailer name, PascalCase or snake_case (e.g. `User`; a `Mailer` suffix is dropped).
        name: String,
        /// Email names in snake_case (e.g. `welcome`), one function each.
        #[arg(required = true)]
        actions: Vec<String>,
    },
    /// `src/mailbox.rs` for incoming email (Cloudflare Email Routing), wired to
    /// the Worker's `email` event in src/lib.rs. One per app.
    ///
    /// Example: `ocre g mailbox`.
    Mailbox,
    /// Background job: `src/jobs/<name>.rs` (arguments, `perform_later` and
    /// `perform`), added to the `Job` enum and `perform` match in
    /// `src/jobs/mod.rs`. The first job wires the `JOBS` queue (cloudflare.config.ts)
    /// and the `queue` event (src/lib.rs); `--queue` adds a named queue.
    ///
    /// Example: `ocre g job SendWelcome user_id:integer`, then
    /// `SendWelcome { user_id }.perform_later(&ctx).await?`.
    Job {
        /// Job name, PascalCase or snake_case (e.g. `SendWelcome`; a `Job` suffix is dropped).
        name: String,
        /// Arguments as `name:type` (see `ocre g model --help`; no `^`).
        fields: Vec<String>,
        /// Queue the job is sent to (lowercase, e.g. `urgent`): its own Cloudflare
        /// queue and consumer, so it never waits behind the `default` queue.
        #[arg(long)]
        queue: Option<String>,
    },
    /// Scheduled task: `src/schedules/<name>.rs`, run by a Cron Trigger (UTC)
    /// added to the triggers of cloudflare.config.ts, dispatched by cron in
    /// `src/schedules/mod.rs`. The first one wires the `scheduled` event.
    ///
    /// Example: `ocre g schedule nightly_cleanup "every day at 3am"` or `ocre g schedule nightly_cleanup "0 3 * * *"`.
    Schedule {
        /// Task name in snake_case (e.g. `nightly_cleanup`).
        name: String,
        /// When, in UTC, quoted: plain English ("every 15 minutes", "every monday at 9am",
        /// "midnight on tuesdays") or five cron fields ("*/15 * * * *").
        cron: String,
    },
    /// `CACHE` Workers KV binding in cloudflare.config.ts for `ocre::cache::fetch`
    /// (read-through cache of JSON values). `ocre deploy` creates the namespace.
    ///
    /// Example: `ocre g cache`.
    Cache,
    /// A browser test (Rails' system test): tests/system/<name>.spec.ts, run by
    /// Playwright against the test server of `ocre test --e2e`. The first one
    /// adds playwright.config.ts and `@playwright/test` to package.json.
    ///
    /// Example: `ocre g system_test signing_up`.
    #[command(name = "system_test", alias = "system-test")]
    SystemTest {
        /// Name in snake_case (e.g. `signing_up`).
        name: String,
    },
    /// Read-only data shipped with the Worker: data/<name>.json, parsed once
    /// per Worker instance by src/data/<name>.rs (`crate::data::<name>::all()`).
    ///
    /// Example: `ocre g data countries`.
    Data {
        /// Name in snake_case (e.g. `countries`).
        name: String,
    },
    /// `.github/workflows/ci.yml`: the checks of `ocre ci` on every push and
    /// pull request, then `ocre deploy` on pushes to main (needs the
    /// CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID repository secrets).
    ///
    /// Example: `ocre g ci`.
    Ci,
    /// Progressive Web App: manifest.webmanifest, service-worker.js, pwa.js and
    /// icon.svg in the assets directory, linked from templates/layout.html.
    ///
    /// Example: `ocre g pwa`.
    Pwa,
    /// `locales/<code>.yml` for each code, declared in `ocre::locales!(...)`
    /// in src/lib.rs. The first run sets up translations: its first code is
    /// the default locale, and routes() gets the `I18n` extractor's layer.
    ///
    /// Example: `ocre g locale en fr`.
    Locale {
        /// Locale codes: `en`, `fr`, `pt-BR`...
        #[arg(required = true)]
        codes: Vec<String>,
    },
    /// Controller with GET actions: `src/<name>.rs` and a page per action in
    /// `templates/<name>/` (HTML), or JSON under /api/<name> in an API-only
    /// app or with --api. Without actions: `index`.
    ///
    /// Example: `ocre g controller Pages about contact` (GET /pages/about, /pages/contact).
    Controller {
        /// Controller name, PascalCase or snake_case (e.g. `Pages`; a `Controller` suffix is dropped).
        name: String,
        /// Action names in snake_case; `index` answers at the controller's root path.
        actions: Vec<String>,
        /// JSON actions under /api/<name> in a full-stack app.
        #[arg(long)]
        api: bool,
        /// Signed-in users only (`CurrentUser`, or `BearerUser` for JSON); needs `ocre g auth`.
        #[arg(long)]
        auth: bool,
    },
    /// Model plus a controller with `index` and `show` over it (HTML pages, or
    /// JSON in an API-only app or with --api): lighter than scaffold, to fill in.
    ///
    /// Example: `ocre g resource Post title:string body:text`.
    Resource {
        /// Singular model name, PascalCase or snake_case (e.g. `BlogPost`).
        name: String,
        /// Fields as `name:type` (see `ocre g model --help`).
        #[arg(required = true)]
        fields: Vec<String>,
        /// JSON actions under /api/<plural> in a full-stack app.
        #[arg(long)]
        api: bool,
    },
    /// Copy generator templates into .ocre/templates/, where they replace the
    /// built-in ones until deleted; without paths, list them.
    ///
    /// Example: `ocre g override controller/view.html`, or `ocre g override controller`.
    Override {
        /// Template paths, or generator names for all their templates.
        paths: Vec<String>,
    },
    /// An app generator in .ocre/generators/<name>/, run as `ocre g <name> <Name> [args]`:
    /// templated files (paths too) and lines to insert after markers.
    ///
    /// Example: `ocre g generator service`, then `ocre g service Billing`.
    Generator {
        /// Generator name in snake_case (e.g. `service`).
        name: String,
    },
    /// Any other name runs the app generator in .ocre/generators/<name>/.
    #[command(external_subcommand)]
    Custom(Vec<String>),
}

#[derive(Subcommand)]
enum SchedulesCommand {
    /// Fire a scheduled task now on the running `ocre dev`, through the
    /// local `/cdn-cgi/local/scheduled` endpoint (Cron Triggers only fire on
    /// the deployed Worker).
    ///
    /// Example: `ocre schedules run nightly_cleanup`.
    Run {
        /// Task name, as in src/schedules/<task>.rs.
        task: String,
        /// Port `ocre dev` listens on.
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
}

#[derive(Subcommand)]
enum I18nCommand {
    /// List keys of the default locale missing from other locales (with the
    /// plural forms each language needs) and invalid locale files. Fails
    /// when there is any.
    Missing,
}

#[derive(Subcommand)]
enum SecretsCommand {
    /// Secret names in .dev.vars and on the deployed Worker, side by side.
    List,
    /// Upload secrets to the deployed Worker, values read from --file (one
    /// `cf workers secrets bulk` call, then the temporary file is deleted).
    Push {
        /// Names of the secrets to upload.
        names: Vec<String>,
        /// `NAME=value` file to read the values from (git-ignored).
        #[arg(long, default_value = ".dev.vars")]
        file: String,
    },
    /// Print one value of a local `NAME=value` file (like `rails credentials:fetch`),
    /// for scripts. Deployed values cannot be read back.
    ///
    /// Example: `ocre secrets fetch STRIPE_KEY --file .prod.vars`.
    Fetch {
        /// Name of the value.
        name: String,
        /// `NAME=value` file to read.
        #[arg(long, default_value = ".dev.vars")]
        file: String,
    },
}

#[derive(Subcommand)]
enum DomainsCommand {
    /// Add a custom domain, e.g. `www.example.com` (its zone must be on the account).
    Add { host: String },
    /// Remove a custom domain.
    Remove { host: String },
}

#[derive(Subcommand)]
enum DbCommand {
    /// Load the fixtures of db/fixtures, then run db/seeds.sql (local database unless --remote).
    ///
    /// Fixtures empty their tables before inserting, so they only load locally.
    Seed {
        /// Seed the production database on Cloudflare (db/seeds.sql only).
        #[arg(long)]
        remote: bool,
        /// Local only: empty every table first (keeps the tables and migrations). Loco's `--reset`.
        #[arg(long)]
        replant: bool,
        /// Fixture directory, relative to the app root [default: db/fixtures].
        #[arg(long, value_name = "DIR")]
        from: Option<String>,
    },
    /// Create the local database, or with --remote the D1 database on Cloudflare when missing.
    Create {
        #[arg(long)]
        remote: bool,
    },
    /// Local only: delete the local database (`ocre db prepare` recreates it).
    Drop {
        /// Refused: Ocre never deletes production data.
        #[arg(long, hide = true)]
        remote: bool,
    },
    /// Print the last applied migration (local unless --remote).
    Version {
        #[arg(long)]
        remote: bool,
    },
    /// Local only: delete every row of every table, keeping tables and applied migrations.
    Truncate {
        /// Refused: Ocre never deletes production data.
        #[arg(long, hide = true)]
        remote: bool,
    },
    /// Local, safe to repeat: apply pending migrations; seed when the database was just created.
    Prepare,
    /// Local only: delete the local database, apply every migration, then load db/fixtures and db/seeds.sql if present.
    Reset,
    /// Write the database's CREATE statements to db/schema.sql (local unless --remote).
    ///
    /// A snapshot to read, and the input of `ocre g migration rebuild_<table>`.
    Schema {
        /// Dump the production database on Cloudflare.
        #[arg(long)]
        remote: bool,
    },
    /// Write table rows to fixture files `<dir>/<table>.yml` that `ocre db seed` loads back (local unless --remote).
    Dump {
        /// Tables to dump, comma-separated [default: every app table].
        #[arg(long, value_delimiter = ',', value_name = "TABLES")]
        tables: Vec<String>,
        /// Directory of the fixture files, relative to the app root.
        #[arg(long, default_value = "db/fixtures")]
        dir: String,
        /// Overwrite existing fixture files.
        #[arg(long)]
        force: bool,
        /// Dump the production database on Cloudflare (read-only).
        #[arg(long)]
        remote: bool,
    },
}

/// `--flag` / `--no-flag` pair: `None` when neither was given.
fn toggle(on: bool, off: bool) -> Option<bool> {
    if on {
        Some(true)
    } else if off {
        Some(false)
    } else {
        None
    }
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();
    if let Command::Generate(args) = &mut cli.command {
        args.take_custom_flags(&mut cli.json);
    }
    let json = cli.json;
    let result = match cli.command {
        Command::New {
            name,
            api,
            full_stack,
            starter,
            login,
            no_login,
            account_id,
            git,
            no_git,
            deploy,
            no_deploy,
            no_install,
            yes,
            ocre_path,
            template,
        } => {
            let args = NewArgs {
                name,
                ocre_path,
                api: toggle(api, full_stack),
                starter,
                account_id,
                git: toggle(git, no_git),
                login: toggle(login, no_login),
                deploy: toggle(deploy, no_deploy),
                yes,
                install: !no_install,
                template,
            };
            new::run(args, json)
        }
        Command::Login => cloudflare::login(json),
        Command::Generate(args) => generate_command(args),
        Command::Destroy { generator, name, force, pretend } => {
            Project::find().and_then(|project| destroy::destroy(&project, &generator, name.as_deref(), force, pretend))
        }
        Command::I18n(I18nCommand::Missing) => Project::find().and_then(|project| i18n::missing(&project)),
        Command::Migrate { remote, status: true } => db::status(remote, json),
        Command::Migrate { remote, status: false } => cloudflare::migrate(remote, json),
        Command::Db(DbCommand::Seed { remote, replant, from }) => db::seed(remote, replant, from.as_deref(), json),
        Command::Db(DbCommand::Create { remote }) => db_admin::create(remote, json),
        Command::Db(DbCommand::Drop { remote }) => db_admin::drop(remote),
        Command::Db(DbCommand::Version { remote }) => db_admin::version(remote),
        Command::Db(DbCommand::Truncate { remote }) => db_admin::truncate(remote, json),
        Command::Db(DbCommand::Prepare) => db_admin::prepare(json),
        Command::Template { source } => template::run(&source),
        Command::Version => about::version(),
        Command::About => about::about(),
        Command::Doctor => doctor::doctor(),
        Command::Ci { signoff } => Project::find().and_then(|project| ci::run(&project, signoff, json)),
        Command::Domains { action } => Project::find().and_then(|project| {
            let action = match action {
                None => domains::Action::List,
                Some(DomainsCommand::Add { host }) => domains::Action::Add(host),
                Some(DomainsCommand::Remove { host }) => domains::Action::Remove(host),
            };
            domains::run(&project, action)
        }),
        Command::Stats { dirs } => Project::find().and_then(|project| stats::run(&project, &dirs)),
        Command::Notes { annotations } => Project::find().and_then(|project| notes::run(&project, &annotations)),
        Command::Test { e2e, port, cargo_args } => {
            Project::find().and_then(|project| testing::run(&project, e2e, port, &cargo_args, json))
        }
        Command::Db(DbCommand::Reset) => db::reset(json),
        Command::Db(DbCommand::Schema { remote }) => db::schema(remote, json),
        Command::Db(DbCommand::Dump { tables, dir, force, remote }) => db::dump(&tables, &dir, force, remote),
        Command::Sql { query, remote } => db::sql(&query, remote, json),
        Command::Dev { port, cache, no_cache } => cloudflare::dev(port, toggle(cache, no_cache), json),
        Command::Deploy => cloudflare::deploy(json),
        Command::Logs { format, status, search } => cloudflare::logs(&format, &status, search.as_deref(), json),
        Command::Secret => secret::run(),
        Command::Secrets(SecretsCommand::List) => Project::find().and_then(|project| secrets::list(&project, json)),
        Command::Secrets(SecretsCommand::Push { names, file }) => {
            Project::find().and_then(|project| secrets::push(&project, &names, &file, json))
        }
        Command::Secrets(SecretsCommand::Fetch { name, file }) => {
            Project::find().and_then(|project| secrets::fetch(&project, &name, &file))
        }
        Command::Routes { filter } => Project::find().and_then(|project| routes::run(&project, filter.as_deref())),
        Command::Schedules { action: None } => Project::find().and_then(|project| schedules::list(&project)),
        Command::Schedules { action: Some(SchedulesCommand::Run { task, port }) } => {
            Project::find().and_then(|project| schedules::run(&project, &task, port))
        }
    };
    output::finish(result, json)
}

/// Runs a generator with the `--pretend`/`--force`/`--skip` flags.
fn generate_command(args: GenerateArgs) -> CliResult {
    let mut project = Project::find()?;
    project.generate = GenerateOptions {
        pretend: args.pretend,
        existing: if args.force {
            Existing::Force
        } else if args.skip {
            Existing::Skip
        } else {
            Existing::Fail
        },
        invocation: std::env::args().skip(1).collect(),
    };
    let project = &project;
    match args.command {
        GenerateCommand::Scaffold { name, fields, realtime } => {
            if project.api_only && realtime {
                Err(CliError::new("--realtime updates HTML pages; this app is API-only").hint(
                    "run `ocre g scaffold` without --realtime; to push JSON to clients, see Realtime in the Ocre README",
                ))
            } else if project.api_only {
                generate::api(project, &name, &fields, false)
            } else {
                generate::scaffold(project, &name, &fields, realtime)
            }
        }
        GenerateCommand::Api { name, fields, graphql } => generate::api(project, &name, &fields, graphql),
        GenerateCommand::Auth { db_sessions, oauth } => {
            generate::auth(project, &generate::AuthOptions { db_sessions, oauth })
        }
        GenerateCommand::Migration { name, fields } => generate::migration(project, &name, &fields),
        GenerateCommand::Model { name, fields } => generate::model(project, &name, &fields),
        GenerateCommand::Mailer { name, actions } => generate::mailer(project, &name, &actions),
        GenerateCommand::Mailbox => generate::mailbox(project),
        GenerateCommand::Job { name, fields, queue } => generate::job(project, &name, &fields, queue.as_deref()),
        GenerateCommand::Schedule { name, cron } => generate::schedule(project, &name, &cron),
        GenerateCommand::Cache => generate::cache(project),
        GenerateCommand::Data { name } => generate::data(project, &name),
        GenerateCommand::SystemTest { name } => generate::system_test(project, &name),
        GenerateCommand::Ci => generate::ci(project),
        GenerateCommand::Pwa => generate::pwa(project),
        GenerateCommand::Locale { codes } => generate::locale(project, &codes),
        GenerateCommand::Controller { name, actions, api, auth } => {
            generate::controller(project, &name, &actions, api, auth)
        }
        GenerateCommand::Resource { name, fields, api } => generate::resource(project, &name, &fields, api),
        GenerateCommand::Override { paths } => generate::override_templates(project, &paths),
        GenerateCommand::Generator { name } => generate::generator(project, &name),
        GenerateCommand::Custom(args) => generate::custom(project, &args[0], &args[1..]),
    }
}

/// Shorthand used by every command.
pub type CliResult = Result<Report, CliError>;
