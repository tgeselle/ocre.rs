//! `ocre`: command-line tool for Ocre apps.
//!
//! Commands never prompt unless `ocre new` runs in a terminal without
//! `--json`/`--yes`. With `--json`, stdout carries exactly one JSON object
//! (`{"ok": true, ...}` or `{"ok": false, "error", "hint"}`) and all tool
//! output (wrangler, cargo) goes to stderr.

mod db;
mod generate;
mod i18n;
mod names;
mod new;
mod output;
mod project;
mod routes;
mod secret;
mod wizard;
mod wrangler;

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};

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
        /// Never prompt; use defaults for anything not given.
        #[arg(long, short)]
        yes: bool,
        /// Use a local checkout of the `ocre` crate instead of the git dependency.
        #[arg(long)]
        ocre_path: Option<PathBuf>,
    },
    /// Log in to Cloudflare (opens a browser) unless already logged in.
    Login,
    /// Generate code (alias: `g`).
    #[command(alias = "g", subcommand)]
    Generate(GenerateCommand),
    /// Apply D1 migrations (local database unless --remote).
    Migrate {
        /// Apply to the production database on Cloudflare.
        #[arg(long)]
        remote: bool,
        /// List pending migrations instead of applying them.
        #[arg(long)]
        status: bool,
    },
    /// Database tasks: seed, reset.
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
    /// Apply local migrations, then run the app with `wrangler dev`.
    Dev {
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
    /// Deploy to Cloudflare and apply remote migrations.
    Deploy,
    /// Print a new random secret, like `rails secret`: a value for SECRET_KEY_BASE.
    ///
    /// Example: `ocre secret | npx wrangler secret put SECRET_KEY_BASE`.
    Secret,
    /// List the app's HTTP routes, read from its source (no build).
    ///
    /// Example: `ocre routes posts` keeps routes whose method, path or handler contains "posts".
    Routes {
        /// Only routes whose method, path or handler contains this text (case-insensitive).
        filter: Option<String>,
    },
    /// Translations: checks of the locale files (see `ocre g locale`).
    #[command(subcommand)]
    I18n(I18nCommand),
}

#[derive(Subcommand)]
enum GenerateCommand {
    /// Table, migration and `src/models/<model>.rs` (queries, validations,
    /// associations). Scaffold and api create the model when it is missing.
    ///
    /// Example: `ocre g model Post title:string^ body:text author:references`.
    /// Types: string, text, integer, float, boolean, date, datetime, references;
    /// suffix `?` for optional, `^` for unique.
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
    /// full-stack apps sign-up/login/logout pages, password reset and magic-link
    /// login by email, `CurrentUser`/`OptionalUser` extractors (src/auth.rs);
    /// in every app a JSON API with JWTs and API keys and the `BearerUser`
    /// extractor (src/auth_api.rs). Runs once per app.
    ///
    /// Example: `ocre g auth`, then `ocre migrate`.
    Auth,
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
    /// Background job: `src/jobs/<name>.rs` (arguments + `perform`), added to
    /// the `Job` enum and `perform` match in `src/jobs/mod.rs`. The first job
    /// wires the `JOBS` queue (wrangler.toml) and the `queue` event (src/lib.rs).
    ///
    /// Example: `ocre g job SendWelcome user_id:integer`, then
    /// `ocre::jobs::enqueue(&ctx, &Job::SendWelcome(SendWelcome { user_id })).await?`.
    Job {
        /// Job name, PascalCase or snake_case (e.g. `SendWelcome`; a `Job` suffix is dropped).
        name: String,
        /// Arguments as `name:type` (see `ocre g model --help`; no `^`).
        fields: Vec<String>,
    },
    /// Scheduled task: `src/schedules/<name>.rs`, run by a Cron Trigger (UTC)
    /// added to `[triggers] crons` in wrangler.toml, dispatched by cron in
    /// `src/schedules/mod.rs`. The first one wires the `scheduled` event.
    ///
    /// Example: `ocre g schedule nightly_cleanup "0 3 * * *"`.
    Schedule {
        /// Task name in snake_case (e.g. `nightly_cleanup`).
        name: String,
        /// Cron expression, five fields in UTC, quoted (e.g. "*/15 * * * *").
        cron: String,
    },
    /// `CACHE` Workers KV binding in wrangler.toml for `ocre::cache::fetch`
    /// (read-through cache of JSON values). `ocre deploy` creates the namespace.
    ///
    /// Example: `ocre g cache`.
    Cache,
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
}

#[derive(Subcommand)]
enum I18nCommand {
    /// List keys of the default locale missing from other locales (with the
    /// plural forms each language needs) and invalid locale files. Fails
    /// when there is any.
    Missing,
}

#[derive(Subcommand)]
enum DbCommand {
    /// Run db/seeds.sql (local database unless --remote).
    Seed {
        /// Seed the production database on Cloudflare.
        #[arg(long)]
        remote: bool,
    },
    /// Local only: delete the local database, apply every migration, then run db/seeds.sql if present.
    Reset,
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
    let cli = Cli::parse();
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
            yes,
            ocre_path,
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
            };
            new::run(args, json)
        }
        Command::Login => wrangler::login(json),
        Command::Generate(GenerateCommand::Scaffold { name, fields, realtime }) => Project::find().and_then(|project| {
            if project.api_only && realtime {
                Err(CliError::new("--realtime updates HTML pages; this app is API-only").hint(
                    "run `ocre g scaffold` without --realtime; to push JSON to clients, see Realtime in the Ocre README",
                ))
            } else if project.api_only {
                generate::api(&project, &name, &fields, false)
            } else {
                generate::scaffold(&project, &name, &fields, realtime)
            }
        }),
        Command::Generate(GenerateCommand::Api { name, fields, graphql }) => {
            Project::find().and_then(|project| generate::api(&project, &name, &fields, graphql))
        }
        Command::Generate(GenerateCommand::Auth) => Project::find().and_then(|project| generate::auth(&project)),
        Command::Generate(GenerateCommand::Migration { name, fields }) => {
            Project::find().and_then(|project| generate::migration(&project, &name, &fields))
        }
        Command::Generate(GenerateCommand::Model { name, fields }) => {
            Project::find().and_then(|project| generate::model(&project, &name, &fields))
        }
        Command::Generate(GenerateCommand::Mailer { name, actions }) => {
            Project::find().and_then(|project| generate::mailer(&project, &name, &actions))
        }
        Command::Generate(GenerateCommand::Mailbox) => Project::find().and_then(|project| generate::mailbox(&project)),
        Command::Generate(GenerateCommand::Job { name, fields }) => {
            Project::find().and_then(|project| generate::job(&project, &name, &fields))
        }
        Command::Generate(GenerateCommand::Schedule { name, cron }) => {
            Project::find().and_then(|project| generate::schedule(&project, &name, &cron))
        }
        Command::Generate(GenerateCommand::Cache) => Project::find().and_then(|project| generate::cache(&project)),
        Command::Generate(GenerateCommand::Locale { codes }) => {
            Project::find().and_then(|project| generate::locale(&project, &codes))
        }
        Command::I18n(I18nCommand::Missing) => Project::find().and_then(|project| i18n::missing(&project)),
        Command::Migrate { remote, status: true } => db::status(remote, json),
        Command::Migrate { remote, status: false } => wrangler::migrate(remote, json),
        Command::Db(DbCommand::Seed { remote }) => db::seed(remote, json),
        Command::Db(DbCommand::Reset) => db::reset(json),
        Command::Sql { query, remote } => db::sql(&query, remote, json),
        Command::Dev { port } => wrangler::dev(port, json),
        Command::Deploy => wrangler::deploy(json),
        Command::Secret => secret::run(),
        Command::Routes { filter } => Project::find().and_then(|project| routes::run(&project, filter.as_deref())),
    };
    output::finish(result, json)
}

/// Shorthand used by every command.
pub type CliResult = Result<Report, CliError>;
