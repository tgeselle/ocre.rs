//! `ocre`: command-line tool for Ocre apps.
//!
//! Commands never prompt unless `ocre new` runs in a terminal without
//! `--json`/`--yes`. With `--json`, stdout carries exactly one JSON object
//! (`{"ok": true, ...}` or `{"ok": false, "error", "hint"}`) and all tool
//! output (wrangler, cargo) goes to stderr.

mod generate;
mod names;
mod new;
mod output;
mod project;
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
    },
    /// Apply local migrations, then run the app with `wrangler dev`.
    Dev {
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
    /// Deploy to Cloudflare and apply remote migrations.
    Deploy,
}

#[derive(Subcommand)]
enum GenerateCommand {
    /// CRUD resource: migration, model, handlers, routes and templates.
    ///
    /// Example: `ocre g scaffold Post title:string body:text published:boolean`.
    /// Field types: string, text, integer, float, boolean.
    Scaffold {
        /// Singular model name, PascalCase or snake_case (e.g. `BlogPost`).
        name: String,
        /// Fields as `name:type`.
        #[arg(required = true)]
        fields: Vec<String>,
    },
    /// Empty numbered SQL migration file.
    Migration {
        /// Migration name in snake_case (e.g. `add_slug_to_posts`).
        name: String,
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
    let cli = Cli::parse();
    let json = cli.json;
    let result = match cli.command {
        Command::New { name, starter, login, no_login, account_id, git, no_git, deploy, no_deploy, yes, ocre_path } => {
            let args = NewArgs {
                name,
                ocre_path,
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
        Command::Generate(GenerateCommand::Scaffold { name, fields }) => {
            Project::find().and_then(|project| generate::scaffold(&project, &name, &fields))
        }
        Command::Generate(GenerateCommand::Migration { name }) => {
            Project::find().and_then(|project| generate::migration(&project, &name))
        }
        Command::Migrate { remote } => wrangler::migrate(remote, json),
        Command::Dev { port } => wrangler::dev(port, json),
        Command::Deploy => wrangler::deploy(json),
    };
    output::finish(result, json)
}

/// Shorthand used by every command.
pub type CliResult = Result<Report, CliError>;
