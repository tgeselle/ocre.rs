//! `ocre`: command-line tool for Ocre apps.
//!
//! Commands never prompt unless `ocre new` runs in a terminal without
//! `--json`/`--yes`. With `--json`, stdout carries exactly one JSON object
//! (`{"ok": true, ...}` or `{"ok": false, "error", "hint"}`) and all tool
//! output (wrangler, cargo) goes to stderr.

mod db;
mod generate;
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
    /// Example: `ocre g scaffold Post title:string body:text published:boolean`.
    Scaffold {
        /// Singular model name, PascalCase or snake_case (e.g. `BlogPost`).
        name: String,
        /// Fields as `name:type` (see `ocre g model --help`).
        #[arg(required = true)]
        fields: Vec<String>,
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
        Command::Generate(GenerateCommand::Scaffold { name, fields }) => Project::find().and_then(|project| {
            if project.api_only {
                generate::api(&project, &name, &fields, false)
            } else {
                generate::scaffold(&project, &name, &fields)
            }
        }),
        Command::Generate(GenerateCommand::Api { name, fields, graphql }) => {
            Project::find().and_then(|project| generate::api(&project, &name, &fields, graphql))
        }
        Command::Generate(GenerateCommand::Migration { name, fields }) => {
            Project::find().and_then(|project| generate::migration(&project, &name, &fields))
        }
        Command::Generate(GenerateCommand::Model { name, fields }) => {
            Project::find().and_then(|project| generate::model(&project, &name, &fields))
        }
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
